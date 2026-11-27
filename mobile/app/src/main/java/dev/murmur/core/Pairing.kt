package dev.murmur.core

import android.util.Base64
import android.util.Log
import com.goterl.lazysodium.LazySodiumAndroid
import com.goterl.lazysodium.SodiumAndroid
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

/** What pairing hands back — everything needed to join the family. */
data class Provisioning(
    val deviceId: String,
    val tokenHex: String,
    val familyKey: ByteArray,
    val serverUrl: String,
    val keyEpoch: Long,
    val familyName: String?,
    /** "serverful" | "serverless" — stored explicitly, never inferred. */
    val familyKind: String,
)

class PairingException(message: String) : Exception(message)

/** Where the joiner is in the handshake — drives the UI's "what to do
 *  on the laptop now" hints. */
sealed interface PairStage {
    /** Socket up, PairHello sent — the inviter has to accept. */
    data class AwaitingAccept(val session: String) : PairStage
    /** SAS matched here — the inviter has to confirm on their side. */
    data class AwaitingConfirm(val session: String) : PairStage
}

/**
 * The joiner side of pairing — one suspend call:
 *
 *   invite → connect (role=pair, conn id = session) → PairHello →
 *   PairAccept → DH → `onSas(code)` for human verify → PairData →
 *   DH-sealed credentials.
 *
 * `onSas` shows the six-digit code; returning false aborts (SAS mismatch
 * = MITM). Throws [PairingException] on any protocol/transport failure.
 */
suspend fun pair(
    inviteCode: String,
    deviceName: String,
    serverUrl: String? = null,
    onStage: (PairStage) -> Unit = {},
    onSas: suspend (code: String) -> Boolean,
): Provisioning {
    // Full invite carries server+session; a typeable XXXX-XXXX code
    // derives the session locally and needs the server passed in.
    val invite = Invite.decode(inviteCode)
    val session: String
    val url: String
    if (invite != null) {
        if (invite.kind == "serverless") {
            // `serverUrl` is `host:port` of the inviter's ephemeral
            // listener — dial it directly; there is no relay.
            return pairLan(invite.serverUrl, invite.session, deviceName, onStage, onSas)
        }
        session = invite.session
        url = invite.serverUrl
    } else {
        session = sessionFromCode(inviteCode.trim())
        url = serverUrl?.trim()?.takeIf { it.isNotEmpty() }
            ?.let { if ("://" in it) it else "wss://$it" }
            ?: throw PairingException("short codes need a server address")
    }
    val sodium = LazySodiumAndroid(SodiumAndroid())
    val pairKey = PairKey(sodium)
    var seq = 1L

    // (envelope-from, control) — `from` carries the inviter id the SAS
    // needs on the short-code path.
    val frames = Channel<Pair<String, Control>>(Channel.UNLIMITED)
    val failure = CompletableDeferred<PairingException>()
    lateinit var ws: WebSocket

    val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            webSocket.send(
                protoJson.encodeToString(
                    Control.serializer(),
                    Control.Hello(
                        device_id = session,
                        name = deviceName,
                        role = "pair",
                        proto_v = Envelope.VERSION,
                    ),
                ),
            )
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            // First frame is a raw Control (HelloOk/Err); after auth it's
            // envelopes carrying cleartext Control in the payload.
            runCatching { protoJson.decodeFromString<Control>(text) }.getOrNull()?.let {
                frames.trySend("" to it)
                return
            }
            runCatching { Envelope.fromJson(text) }.getOrNull()?.let { env ->
                env.controlPayload()?.let { frames.trySend(env.from to it) }
            }
        }

        private fun fail(msg: String) {
            failure.complete(PairingException(msg))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) =
            fail("connection closed: $reason")

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) =
            fail(t.message ?: "connection failed")
    }

    ws = OkHttpClient().newWebSocket(Request.Builder().url(url).build(), listener)
    try {
        suspend fun next(): Pair<String, Control> {
            val f = kotlinx.coroutines.selects.select {
                frames.onReceive { it }
                failure.onAwait { throw it }
            }
            // The relay answers a PairHello for an unknown/expired
            // session with PairReject — fail fast instead of waiting
            // out the timeout for an inviter that will never come.
            (f.second as? Control.PairReject)?.let { throw PairingException(it.reason) }
            return f
        }

        return withTimeout(TIMEOUT_MS) {
            when (val first = next().second) {
                is Control.HelloOk -> Unit
                is Control.HelloErr -> throw PairingException("server: ${first.reason}")
                else -> throw PairingException("expected hello_ok")
            }

            // PairHello → @server, which forwards it to the inviter that
            // PairOpen'd this session — the joiner needs no inviter id.
            // Resend every few seconds until something answers: if the
            // hello landed before the inviter's PairOpen (slow relay,
            // invite typed before the laptop connected), a single shot
            // just drops and both sides wait forever.
            val hello = {
                ws.send(
                    Envelope.wrap(
                        from = session,
                        to = Envelope.SERVER,
                        seq = seq++,
                        ctl = Control.PairHello(
                            pubkey_b64 = pairKey.publicB64,
                            name = deviceName,
                        ),
                    ).toJson(),
                )
            }
            onStage(PairStage.AwaitingAccept(session))

            var inviter = ""
            var frame: Control? = null
            while (frame !is Control.PairAccept) {
                hello()
                // PairReject throws inside next(); a null here is just the
                // retry window elapsing — resend. The outer withTimeout
                // still bounds the whole handshake.
                val got = withTimeoutOrNull(HELLO_RETRY_MS) { next() }
                if (got != null) {
                    inviter = got.first
                    frame = got.second
                }
            }
            // The relay itself answering = this was the one-shot
            // bootstrap code. The founder has to be a laptop (it mints
            // the family key and confirms against the relay log).
            if (inviter == ZERO_ID) {
                throw PairingException(
                    "that's the relay's bootstrap code — pair a laptop first, " +
                        "then run murmurctl invite on it",
                )
            }
            Log.d(TAG, "pair accepted, computing sas")
            val accept = frame as? Control.PairAccept
                ?: throw PairingException("expected pair_accept")
            val peerPub = Base64.decode(accept.pubkey_b64, Base64.NO_WRAP)
            val dh = pairKey.dh(peerPub)
            val sas = sasCode(
                pairKey.public,
                peerPub,
                hexToBytes(session),
                hexToBytes(inviter),
            )
            if (!onSas(sas)) {
                throw PairingException("rejected — codes didn't match")
            }
            onStage(PairStage.AwaitingConfirm(session))

            // The inviter sends PairData only after *their* confirmation.
            Log.d(TAG, "pair awaiting pair_data")
            val got = next()
            Log.d(TAG, "pair got post-confirm frame: ${got.second::class.simpleName}")
            val data = got.second as? Control.PairData
                ?: throw PairingException("expected pair_data")
            val blobBytes = Base64.decode(data.blob_b64, Base64.NO_WRAP)
            Log.d(TAG, "pair blob ${blobBytes.size}B, opening")
            val blob = protoJson.decodeFromString<PairBlob>(
                FamilyKey.fromBytes(dh).open(blobBytes).decodeToString(),
            )
            Log.d(TAG, "pair blob opened, device=${blob.device_id}")
            Provisioning(
                deviceId = blob.device_id,
                tokenHex = blob.token_hex,
                familyKey = Base64.decode(blob.family_key_b64, Base64.NO_WRAP),
                // The address that actually carried the pairing wins —
                // the inviter's advertised URL may be unreachable from
                // this network (e.g. a LAN IP behind a firewall).
                serverUrl = url,
                keyEpoch = blob.key_epoch,
                familyName = blob.family_name,
                familyKind = blob.kind,
            )
        }
    } finally {
        ws.close(1000, null)
    }
}

private const val TAG = "MurmurPair"
private const val TIMEOUT_MS = 180_000L
private const val HELLO_RETRY_MS = 3_000L
private const val CONNECT_MS = 10_000
private const val ZERO_ID = "0000000000000000"

/**
 * Serverless short code: `XXXX-XXXX` derives the session (same
 * SHA-256(code)[0..8] the laptop uses), then the inviter's LAN pair
 * beacon resolves it to `host:port` — the stand-in for the relay's
 * session lookup a serverless family doesn't have.
 */
suspend fun pairLanCode(
    code: String,
    deviceName: String,
    onStage: (PairStage) -> Unit = {},
    onSas: suspend (code: String) -> Boolean,
): Provisioning {
    val norm = code.filter { it != '-' }.uppercase()
    val digest = java.security.MessageDigest
        .getInstance("SHA-256")
        .digest(norm.toByteArray())
    val session = digest.take(8).joinToString("") { "%02x".format(it) }
    val addr = try {
        awaitPairBeacon(session)
    } catch (e: TimeoutCancellationException) {
        throw PairingException("no laptop answering that code on this lan")
    }
    return pairLan(addr, session, deviceName, onStage, onSas)
}

private const val BEACON_PORT = 17893
private const val BEACON_WAIT_MS = 120_000L

/** Listen on the beacon port for the inviter's `{"t":"murmur-pair"}`
 *  datagram matching our session — the laptop broadcasts it while the
 *  invite lives. */
private suspend fun awaitPairBeacon(session: String): String = withContext(Dispatchers.IO) {
    val sock = java.net.DatagramSocket(null).apply {
        reuseAddress = true
        bind(java.net.InetSocketAddress(BEACON_PORT))
    }
    try {
        withTimeout(BEACON_WAIT_MS) {
            val buf = ByteArray(2048)
            while (true) {
                val pkt = java.net.DatagramPacket(buf, buf.size)
                sock.receive(pkt)
                val o = runCatching {
                    org.json.JSONObject(String(pkt.data, 0, pkt.length))
                }.getOrNull() ?: continue
                if (o.optString("t") == "murmur-pair" && o.optString("s") == session) {
                    return@withTimeout o.getString("a")
                }
            }
            @Suppress("UNREACHABLE_CODE")
            error("unreachable")
        }
    } finally {
        runCatching { sock.close() }
    }
}

/**
 * Joiner side of a serverless invite: `addr` is the inviter's
 * `host:port`. Same frame semantics as the relay path, carried as
 * newline-delimited JSON envelopes over a raw socket — no `Hello`
 * handshake (the session id in `from` is the introduction), no
 * bootstrap codes (no relay exists to mint them).
 */
private suspend fun pairLan(
    addr: String,
    session: String,
    deviceName: String,
    onStage: (PairStage) -> Unit,
    onSas: suspend (code: String) -> Boolean,
): Provisioning {
    val host = addr.substringBefore(':')
    val port = addr.substringAfter(':').toIntOrNull()
        ?: throw PairingException("bad direct invite: $addr")
    val sodium = LazySodiumAndroid(SodiumAndroid())
    val pairKey = PairKey(sodium)

    val socket = withContext(Dispatchers.IO) {
        java.net.Socket().apply {
            connect(java.net.InetSocketAddress(host, port), CONNECT_MS)
            tcpNoDelay = true
        }
    }
    Log.d(TAG, "pair tcp connected to $addr")
    return try {
        coroutineScope {
            val out = socket.getOutputStream()
            val br = java.io.BufferedReader(java.io.InputStreamReader(socket.getInputStream()))
            val frames = Channel<Pair<String, Control>>(Channel.UNLIMITED)
            val failure = CompletableDeferred<PairingException>()

            launch(Dispatchers.IO) {
                try {
                    while (true) {
                        val line = br.readLine() ?: break
                        Log.d(TAG, "pair rx line ${line.length}B")
                        val env = runCatching { Envelope.fromJson(line) }.getOrNull() ?: run {
                            Log.w(TAG, "pair rx unparsable envelope")
                            continue
                        }
                        val ctl = env.controlPayload()
                        Log.d(TAG, "pair rx ctl=${ctl?.let { it::class.simpleName } ?: "null(dropped)"} from=${env.from}")
                        ctl?.let { frames.trySend(env.from to it) }
                    }
                    Log.w(TAG, "pair socket closed by peer")
                    failure.complete(PairingException("connection closed"))
                } catch (e: java.io.IOException) {
                    Log.w(TAG, "pair socket error: ${e.message}")
                    failure.complete(PairingException(e.message ?: "connection failed"))
                }
            }

            suspend fun sendFrame(env: Envelope) = withContext(Dispatchers.IO) {
                out.write("${env.toJson()}\n".toByteArray())
                out.flush()
            }

            suspend fun next(): Pair<String, Control> {
                val f = select {
                    frames.onReceive { it }
                    failure.onAwait { throw it }
                }
                (f.second as? Control.PairReject)?.let { throw PairingException(it.reason) }
                return f
            }

            // The reader child blocks in `readLine`, which coroutine
            // cancellation can't interrupt — and `coroutineScope` can't
            // complete while a child lives. Whatever way the handshake
            // ends (success, error, cancel, timeout), closing the socket
            // is what actually kills the reader and lets the scope return.
            try {
            withTimeout(TIMEOUT_MS) {
                Log.d(TAG, "pair sending hello, awaiting accept")
                onStage(PairStage.AwaitingAccept(session))
                var seq = 1L
                suspend fun hello() = sendFrame(
                    Envelope.wrap(
                        from = session,
                        to = Envelope.SERVER,
                        seq = seq++,
                        ctl = Control.PairHello(
                            pubkey_b64 = pairKey.publicB64,
                            name = deviceName,
                        ),
                    ),
                )

                var inviter = ""
                var frame: Control? = null
                while (frame !is Control.PairAccept) {
                    hello()
                    val got = withTimeoutOrNull(HELLO_RETRY_MS) { next() }
                    if (got != null) {
                        inviter = got.first
                        frame = got.second
                    }
                }
                Log.d(TAG, "pair accept consumed, computing dh+sas")
                val accept = frame as Control.PairAccept
                val peerPub = Base64.decode(accept.pubkey_b64, Base64.NO_WRAP)
                val dh = pairKey.dh(peerPub)
                val sas = sasCode(
                    pairKey.public,
                    peerPub,
                    hexToBytes(session),
                    hexToBytes(inviter),
                )
                Log.d(TAG, "pair sas=$sas, showing")
                if (!onSas(sas)) {
                    throw PairingException("rejected — codes didn't match")
                }
                Log.d(TAG, "pair sas confirmed by user, awaiting pair_data")
                onStage(PairStage.AwaitingConfirm(session))

                val data = next().second as? Control.PairData
                    ?: throw PairingException("expected pair_data")
                Log.d(TAG, "pair pair_data consumed")
                val blobBytes = Base64.decode(data.blob_b64, Base64.NO_WRAP)
                val blob = protoJson.decodeFromString<PairBlob>(
                    FamilyKey.fromBytes(dh).open(blobBytes).decodeToString(),
                )
                Provisioning(
                    deviceId = blob.device_id,
                    tokenHex = blob.token_hex,
                    familyKey = Base64.decode(blob.family_key_b64, Base64.NO_WRAP),
                    // There is no server to keep — the invite's
                    // `host:port` was a one-shot rendezvous.
                    serverUrl = "",
                    keyEpoch = blob.key_epoch,
                    familyName = blob.family_name,
                    familyKind = "serverless",
                )
            }
            } finally {
                runCatching { socket.close() }
            }
        }
    } finally {
        runCatching { socket.close() }
    }
}
