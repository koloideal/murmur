package dev.murmur.core

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.io.OutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicReference

/**
 * LAN fast-path sibling of [Relay]: sealed-envelope UDP beacon +
 * ServerSocket accept + per-line sealed-JSON envelopes — same wire
 * shape as the WS transport, zero extra auth (a frame that opens =
 * family member).
 *
 * The phone only LISTENS and beacons — laptops dial. [sendIfPeer]
 * short-circuits Device-unicast to the linked peer; everything else
 * (broadcast `*`, other devices) keeps going through the relay.
 */
class LanLink(
    private val selfId: String,
    private val key: FamilyKey,
    private val scope: CoroutineScope,
    private val onMsg: (Envelope) -> Unit,
) {
    private val peer = AtomicReference<String?>(null)

    @Volatile private var out: OutputStream? = null

    @Volatile private var linkSock: Socket? = null

    fun start() {
        scope.launch(Dispatchers.IO) { beaconLoop() }
        scope.launch(Dispatchers.IO) { acceptLoop() }
    }

    fun stop() {
        runCatching { linkSock?.close() }
        peer.set(null)
        out = null
    }

    /** Device id currently behind the link — `null` while unlinked. */
    fun linkedPeer(): String? = peer.get()

    /**
     * Write a sealed envelope over the LAN link iff it can reach the
     * linked peer: unicast to it, or `*` — the linked peer then gets an
     * early copy (the relay still carries the broadcast to others).
     * `false` → caller falls back to the relay.
     */
    fun sendIfPeer(envJson: String, to: String): Boolean {
        val s = out ?: return false
        val p = peer.get() ?: return false
        if (to != p && to != Envelope.ALL) return false
        return runCatching {
            s.write((envJson + "\n").toByteArray())
            s.flush()
            true
        }.getOrElse {
            drop()
            false
        }
    }

    private fun drop() {
        peer.set(null)
        out = null
        runCatching { linkSock?.close() }
        linkSock = null
    }

    // -- beacon ---------------------------------------------------------------

    private suspend fun beaconLoop() {
        val sock = DatagramSocket().apply { broadcast = true }
        val dst = InetSocketAddress("255.255.255.255", BEACON_PORT)
        while (true) {
            val msg = Msg.LanBeacon(LINK_PORT, "phone")
            val pt = protoJson.encodeToString(Msg.serializer(), msg).encodeToByteArray()
            val env = Envelope(
                v = Envelope.VERSION,
                from = selfId,
                to = Envelope.ALL,
                seq = 0,
                payload = android.util.Base64.encodeToString(key.seal(pt), android.util.Base64.NO_WRAP),
            )
            val bytes = env.toJson().encodeToByteArray()
            runCatching { sock.send(DatagramPacket(bytes, bytes.size, dst)) }
            delay(BEACON_MS)
        }
    }

    // -- link -----------------------------------------------------------------

    private fun acceptLoop() {
        val srv = ServerSocket(LINK_PORT)
        while (true) {
            val s = runCatching { srv.accept() }.getOrNull() ?: continue
            s.tcpNoDelay = true
            scope.launch(Dispatchers.IO) { serveLink(s) }
        }
    }

    private fun serveLink(s: Socket) {
        val r = s.getInputStream().bufferedReader()
        while (true) {
            val line = runCatching { r.readLine() }.getOrNull() ?: break
            if (line.isNullOrEmpty()) continue
            val env = runCatching { Envelope.fromJson(line) }.getOrNull() ?: continue
            val pt = runCatching {
                key.open(android.util.Base64.decode(env.payload, android.util.Base64.NO_WRAP))
            }.getOrNull() ?: continue
            val msg = runCatching {
                protoJson.decodeFromString(Msg.serializer(), pt.decodeToString())
            }.getOrNull() ?: continue
            // First frame that opens proves family membership → adopt.
            if (peer.get() == null) {
                peer.set(env.from)
                out = s.getOutputStream()
                linkSock = s
                Log.i(TAG, "lan link up with ${env.from}")
            }
            onMsg(env)
        }
        if (linkSock === s) drop() else runCatching { s.close() }
    }

    private companion object {
        const val TAG = "LanLink"
        const val BEACON_PORT = 17893
        const val LINK_PORT = 17894
        const val BEACON_MS = 4000L
    }
}
