@file:OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)

package dev.murmur.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.onTimeout
import kotlinx.coroutines.selects.select
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlin.math.min
import kotlin.time.Duration.Companion.seconds

/**
 * The deep module: persistent WSS connection to murmurs.
 *
 * Interface for the rest of the app:
 *   val relay = Relay(url, helloJson = { Control.Hello(...) }, scope)
 *   relay.start()
 *   relay.incoming.collect { env -> … }   // raw envelopes in
 *   relay.send(env)                        // raw envelopes out
 *
 * Everything else — reconnect with backoff, auth handshake, queueing while
 * offline — is inside. Callers never see connection state beyond [state].
 */
class Relay(
    private val url: String,
    private val hello: () -> Control.Hello,
    private val scope: CoroutineScope,
    private val onAuthed: suspend () -> Unit = {},
) {
    /** REVOKED is terminal: the relay no longer knows this device. */
    enum class State { CONNECTING, ONLINE, OFFLINE, REVOKED }

    // WS pings keep mobile NATs from silently dropping an idle conn;
    // the server pings too — belt and suspenders for flaky networks.
    private val client = OkHttpClient.Builder()
        .pingInterval(20, TimeUnit.SECONDS)
        .build()
    @Volatile private var ws: WebSocket? = null
    private val running = AtomicBoolean(false)
    // Clock-seeded: after a restart a peer's dedupe window still holds
    // our old low numbers — reusing them silently drops fresh frames.
    private val seq = AtomicLong(System.currentTimeMillis())

    private val _incoming = MutableSharedFlow<Envelope>(extraBufferCapacity = 256)
    val incoming: SharedFlow<Envelope> = _incoming

    private val _state = MutableSharedFlow<State>(replay = 1)
    val state: SharedFlow<State> = _state

    /** Conflated "retry now" — ConnectivityManager pokes this on network
     *  changes so a dead period doesn't wait out the backoff. */
    private val kicks = Channel<Unit>(Channel.CONFLATED)

    fun start() {
        if (running.getAndSet(true)) return
        scope.launch(Dispatchers.IO) { loop() }
    }

    fun stop() {
        running.set(false)
        ws?.close(1000, "shutdown")
    }

    /** Abort the current backoff and reconnect immediately. */
    fun kick() {
        kicks.trySend(Unit)
        ws?.cancel() // dead socket → next loop iteration
    }

    fun send(to: String, payloadJson: String, selfId: String) {
        val env = Envelope(
            v = Envelope.VERSION,
            from = selfId,
            to = to,
            seq = seq.getAndIncrement(),
            payload = payloadJson,
        )
        // Drop silently while offline — live-only by design.
        ws?.send(env.toJson())
    }

    /** Shared sequence so a caller can build the Envelope itself —
     *  needed when the same frame may ride the LAN link instead. */
    fun nextSeq(): Long = seq.getAndIncrement()

    /** Send a pre-built envelope (LAN-prepared). */
    fun sendEnv(env: Envelope) {
        ws?.send(env.toJson())
    }

    /** Cleartext control-plane to the server — device list, rename, revoke. */
    fun sendControl(ctl: Control, selfId: String) {
        ws?.send(Envelope.wrap(selfId, Envelope.SERVER, seq.getAndIncrement(), ctl).toJson())
    }

    private suspend fun loop() {
        var backoff = 1.seconds
        while (running.get()) {
            _state.emit(State.CONNECTING)
            try {
                // Suspends until the socket dies. `true` = we got through auth,
                // so the link itself was healthy → reset backoff.
                if (connectOnce()) backoff = 1.seconds
            } catch (e: RelayAuthException) {
                // Revoked / forgotten by the relay: retrying can't help —
                // stop and let the UI offer a fresh pairing.
                if (e.reason == "revoked" || e.reason == "unknown device") {
                    running.set(false)
                    _state.emit(State.REVOKED)
                    return
                }
            } catch (_: Throwable) {
                // DNS, TLS, network — back off and retry
            }
            _state.emit(State.OFFLINE)
            // Sleep out the backoff unless kicked by a connectivity change.
            select<Unit> {
                kicks.onReceive { }
                onTimeout(backoff) { }
            }
            backoff = min(backoff.inWholeSeconds * 2, 60).seconds
        }
    }

    /**
     * Drives one socket until it dies: send Hello, wait HelloOk, then pump
     * incoming text frames into [_incoming].
     * Returns `true` if auth succeeded before the socket closed.
     */
    private suspend fun connectOnce(): Boolean =
        kotlinx.coroutines.suspendCancellableCoroutine { cont ->
            var authed = false
            val socket = client.newWebSocket(
                Request.Builder().url(url).build(),
                object : WebSocketListener() {
                    override fun onOpen(webSocket: WebSocket, response: Response) {
                        ws = webSocket
                        webSocket.send(protoJson.encodeToString(Control.serializer(), hello()))
                    }

                    override fun onMessage(webSocket: WebSocket, text: String) {
                        if (!authed) {
                            when (val ctl = runCatching {
                                protoJson.decodeFromString(Control.serializer(), text)
                            }.getOrNull()) {
                                is Control.HelloOk -> {
                                    authed = true
                                    scope.launch {
                                        _state.emit(State.ONLINE)
                                        onAuthed()
                                    }
                                }
                                is Control.HelloErr -> {
                                    if (cont.isActive) {
                                        cont.resumeWithException(RelayAuthException(ctl.reason))
                                    }
                                }
                                else -> {}
                            }
                            return
                        }
                        runCatching { Envelope.fromJson(text) }.getOrNull()?.let {
                            if (it.protoMismatch()) {
                                android.util.Log.w("Relay",
                                    "frame from ${it.from} speaks proto v${it.v} — we speak v${Envelope.VERSION}; upgrade the peer")
                                return
                            }
                            _incoming.tryEmit(it)
                        }
                    }

                    override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = done()
                    override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = done()

                    private fun done() {
                        ws = null
                        if (cont.isActive) cont.resume(authed)
                    }
                },
            )
            cont.invokeOnCancellation { socket.cancel() }
        }
}

class RelayAuthException(val reason: String) : Exception("server rejected auth: $reason")
