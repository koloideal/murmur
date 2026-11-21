package dev.murmur.core

import android.util.Base64
import com.goterl.lazysodium.LazySodiumAndroid
import com.goterl.lazysodium.SodiumAndroid
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withTimeout
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
)

class PairingException(message: String) : Exception(message)

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
    onSas: suspend (code: String) -> Boolean,
): Provisioning {
    val invite = Invite.decode(inviteCode)
        ?: throw PairingException("bad invite code")
    val sodium = LazySodiumAndroid(SodiumAndroid())
    val pairKey = PairKey(sodium)
    var seq = 1L

    val frames = Channel<Control>(Channel.UNLIMITED)
    val failure = CompletableDeferred<PairingException>()
    lateinit var ws: WebSocket

    val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            webSocket.send(
                protoJson.encodeToString(
                    Control.serializer(),
                    Control.Hello(
                        device_id = invite.session,
                        name = deviceName,
                        role = "pair",
                    ),
                ),
            )
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            // First frame is a raw Control (HelloOk/Err); after auth it's
            // envelopes carrying cleartext Control in the payload.
            runCatching { protoJson.decodeFromString<Control>(text) }.getOrNull()?.let {
                frames.trySend(it)
                return
            }
            runCatching { Envelope.fromJson(text) }.getOrNull()
                ?.controlPayload()
                ?.let { frames.trySend(it) }
        }

        private fun fail(msg: String) {
            failure.complete(PairingException(msg))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) =
            fail("connection closed: $reason")

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) =
            fail(t.message ?: "connection failed")
    }

    ws = OkHttpClient().newWebSocket(Request.Builder().url(invite.serverUrl).build(), listener)
    try {
        suspend fun next(): Control =
            kotlinx.coroutines.selects.select {
                frames.onReceive { it }
                failure.onAwait { throw it }
            }

        return withTimeout(TIMEOUT_MS) {
            when (val first = next()) {
                is Control.HelloOk -> Unit
                is Control.HelloErr -> throw PairingException("server: ${first.reason}")
                else -> throw PairingException("expected hello_ok")
            }

            ws.send(
                Envelope.wrap(
                    from = invite.session,
                    to = invite.inviter,
                    seq = seq++,
                    ctl = Control.PairHello(
                        pubkey_b64 = pairKey.publicB64,
                        name = deviceName,
                    ),
                ).toJson(),
            )

            val accept = next() as? Control.PairAccept
                ?: throw PairingException("expected pair_accept")
            val peerPub = Base64.decode(accept.pubkey_b64, Base64.NO_WRAP)
            val dh = pairKey.dh(peerPub)
            val sas = sasCode(
                pairKey.public,
                peerPub,
                hexToBytes(invite.session),
                hexToBytes(invite.inviter),
            )
            if (!onSas(sas)) {
                throw PairingException("rejected — codes didn't match")
            }

            // The inviter sends PairData only after *their* confirmation.
            val data = next() as? Control.PairData
                ?: throw PairingException("expected pair_data")
            val blobBytes = Base64.decode(data.blob_b64, Base64.NO_WRAP)
            val blob = protoJson.decodeFromString<PairBlob>(
                FamilyKey.fromBytes(dh).open(blobBytes).decodeToString(),
            )
            Provisioning(
                deviceId = blob.device_id,
                tokenHex = blob.token_hex,
                familyKey = Base64.decode(blob.family_key_b64, Base64.NO_WRAP),
                serverUrl = blob.server_url,
            )
        }
    } finally {
        ws.close(1000, null)
    }
}

private const val TIMEOUT_MS = 180_000L
