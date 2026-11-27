package dev.murmur.core

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.OutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap

/**
 * LAN fast-path sibling of [Relay]: sealed-envelope UDP beacon +
 * ServerSocket accept + per-line sealed-JSON envelopes — same wire
 * shape as the WS transport, zero extra auth (a frame that opens =
 * family member).
 *
 * Laptops dial the phone; the phone also dials back every laptop it
 * has linked with before, at the address that laptop advertised — on
 * broadcast-filtered Wi-Fi a DHCP change of either side would
 * otherwise need a manual `murmurctl dial`. Every peer gets
 * its own entry in [links]: several laptops can hold a link at once
 * (a star with the phone as hub), [sendIfPeer] picks the right link.
 */
class LanLink(
    private val selfId: String,
    private val key: FamilyKey,
    private val scope: CoroutineScope,
    private val onMsg: (Envelope) -> Unit,
    /** Persists [peerAddrs] — a service restart must not forget the
     *  only rendezvous a filtered network leaves us. */
    private val prefs: android.content.SharedPreferences? = null,
    /** Push a sealed message onto the relay — the beacon doubles as
     *  relay-borne discovery because many APs suppress client
     *  broadcast entirely. */
    private val sendRelay: (Msg) -> Unit = {},
) {
    /** One live link to a peer — socket + writer handle + last RTT. */
    private class Link(val sock: Socket, val out: OutputStream) {
        @Volatile var rttMs: Long? = null
    }

    private val links = ConcurrentHashMap<String, Link>()

    /** peer id → where that laptop accepts links, as it advertised in
     *  its in-link beacon. Stored as `id=host:port` lines. */
    private val peerAddrs = ConcurrentHashMap<String, InetSocketAddress>().apply {
        prefs?.getString(KEY_ADDRS, null)?.lineSequence()?.forEach { l ->
            val (id, hp) = l.split('=', limit = 2).takeIf { it.size == 2 } ?: return@forEach
            val host = hp.substringBeforeLast(':')
            val port = hp.substringAfterLast(':').toIntOrNull() ?: return@forEach
            put(id, InetSocketAddress.createUnresolved(host, port))
        }
    }

    private fun rememberAddr(peer: String, host: String, port: Int) {
        val cur = peerAddrs[peer]
        if (cur != null && cur.hostString == host && cur.port == port) return
        peerAddrs[peer] = InetSocketAddress.createUnresolved(host, port)
        prefs?.edit()?.putString(
            KEY_ADDRS,
            peerAddrs.entries.joinToString("\n") { "${it.key}=${it.value.hostString}:${it.value.port}" },
        )?.apply()
    }

    /** The accept socket — held on the class so [stop] can actually
     *  release :17894. Without this a service restart in the same
     *  process leaves a zombie listener that accepts links nobody
     *  serves, and every later instance fails the bind forever. */
    @Volatile private var srv: ServerSocket? = null
    @Volatile private var started = false

    /** peer id → measured LAN round-trip in ms (missing entry = not
     *  linked, null rtt = not yet measured) — drives the per-device
     *  "on lan" badge and the real ping in the UI. */
    val peersFlow = kotlinx.coroutines.flow.MutableStateFlow<Map<String, Long>>(emptyMap())

    private fun publishPeers() {
        peersFlow.value = links.mapValues { it.value.rttMs ?: 0L }
    }

    /** A queued socket write tagged with its target peer. */
    private class Write(val peer: String, val bytes: ByteArray)

    /** One writer owns every link's socket: media/notification
     *  callbacks run on the main thread, where a socket write is a
     *  fatal NetworkOnMainThreadException — a send from any thread is
     *  just a queue push, and writes can't interleave mid-line. */
    private val writeQ = kotlinx.coroutines.channels.Channel<Write>(
        kotlinx.coroutines.channels.Channel.UNLIMITED,
    )

    /** Keepalives jump the data queue — a pong waiting behind a media
     *  burst measures queue depth, not link latency. */
    private val hiQ = kotlinx.coroutines.channels.Channel<Write>(
        kotlinx.coroutines.channels.Channel.UNLIMITED,
    )

    fun start() {
        synchronized(this) {
            if (started) return
            started = true
        }
        supervise("beacon") { beaconLoop() }
        supervise("accept") { acceptLoop() }
        supervise("ping") { pingLoop() }
        supervise("writer") { writerLoop() }
        supervise("dial") { dialLoop() }
    }

    /** Loops are the whole transport — an exception or an accidental
     *  break-out in one must never silence discovery for good, so each
     *  restarts after a breath until [stop]. [stop] flips `started`
     *  first, and both the loop bodies and this wrapper honor it. */
    private fun supervise(name: String, body: suspend () -> Unit) {
        scope.launch(Dispatchers.IO) {
            while (started) {
                runCatching { body() }.onFailure {
                    Log.w(TAG, "$name loop died: ${it.javaClass.simpleName}: ${it.message}")
                }
                if (started) delay(1_000)
            }
        }
    }

    fun stop() {
        started = false
        // Closing the listen socket unblocks accept() — otherwise the
        // coroutine sits on it forever (blocking calls ignore cancel)
        // and the port stays bound across service restarts.
        runCatching { srv?.close() }
        srv = null
        links.values.forEach { runCatching { it.sock.close() } }
        links.clear()
        publishPeers()
    }

    /** Device ids behind live links. */
    fun linkedPeers(): Set<String> = links.keys.toSet()

    /** Our own LAN address — what a laptop `murmurctl dial`s on
     *  broadcast-filtered networks. Refreshed with the beacon cadence. */
    val lanAddrFlow = kotlinx.coroutines.flow.MutableStateFlow<String?>(null)

    /**
     * Queue a sealed envelope over the LAN link(s) it can reach:
     * unicast targets that peer's link, `*` gets an early copy on every
     * link. `false` just means "no early copy"; the caller always sends
     * over the relay regardless.
     */
    fun sendIfPeer(envJson: String, to: String): Boolean {
        val bytes = (envJson + "\n").toByteArray()
        return if (to == Envelope.ALL) {
            if (links.isEmpty()) return false
            for (p in links.keys) writeQ.trySend(Write(p, bytes))
            true
        } else {
            if (!links.containsKey(to)) return false
            writeQ.trySend(Write(to, bytes))
            true
        }
    }

    /** The links' only writer — [hiQ] jumps [writeQ]: a keepalive
     *  never waits behind queued media/notification traffic, so the
     *  RTT reflects the wire, not the backlog. */
    private suspend fun writerLoop() {
        while (true) {
            val w = kotlinx.coroutines.selects.select<Write> {
                hiQ.onReceive { it }
                writeQ.onReceive { it }
            }
            writeOne(w)
            // Drain any hi backlog before blocking again.
            while (true) {
                val h = hiQ.tryReceive().getOrNull() ?: break
                writeOne(h)
            }
        }
    }

    private fun writeOne(w: Write) {
        val l = links[w.peer] ?: return
        runCatching {
            l.out.write(w.bytes)
            l.out.flush()
        }.onFailure {
            drop(w.peer, "write: ${it.javaClass.simpleName}: ${it.message}")
        }
    }

    private fun drop(peer: String, reason: String) {
        Log.i(TAG, "link to $peer dropped ($reason)")
        // Queued writes to the dead peer expire in writerLoop —
        // links[w.peer] is gone by the time they surface.
        links.remove(peer)?.let { runCatching { it.sock.close() } }
        publishPeers()
    }

    /** Seal a message into the link wire shape — same envelope form
     *  the relay carries. */
    private fun sealedEnv(msg: Msg): Envelope {
        val pt = protoJson.encodeToString(Msg.serializer(), msg).encodeToByteArray()
        return Envelope(
            v = Envelope.VERSION,
            from = selfId,
            to = Envelope.ALL,
            seq = 0,
            payload = android.util.Base64.encodeToString(key.seal(pt), android.util.Base64.NO_WRAP),
        )
    }

    /** Periodic RTT probes — each peer echoes LanPong back and its
     *  round-trip lands in the link's rttMs. Also real traffic, so a
     *  truly dead link gets noticed by the reader's idle timeout too. */
    private suspend fun pingLoop() {
        while (true) {
            val env = sealedEnv(Msg.LanPing(System.currentTimeMillis()))
            val bytes = (env.toJson() + "\n").toByteArray()
            for (p in links.keys) hiQ.trySend(Write(p, bytes))
            delay(PING_MS)
        }
    }

    // -- beacon ---------------------------------------------------------------

    private suspend fun beaconLoop() {
        // Socket creation can fail transiently (VPN churn, fd pressure) —
        // retry instead of killing discovery for the session's lifetime.
        val sock = run {
            var s: DatagramSocket? = null
            while (s == null) {
                s = runCatching { DatagramSocket().apply { broadcast = true } }
                    .onFailure { Log.w(TAG, "beacon socket: $it") }.getOrNull()
                if (s == null) delay(SOCKET_RETRY_MS)
            }
            s
        }
        // Limited broadcast PLUS every interface's directed broadcast:
        // on Android a raw 255.255.255.255 send can be routed to the
        // wrong (e.g. cellular) interface, while the subnet-directed
        // address follows the interface's own route. The same beacon
        // also rides the relay — many APs drop client broadcast
        // outright, so L2-independent discovery is the reliable path.
        while (true) {
            val addr = lanAddr()
            lanAddrFlow.value = addr
            val msg = Msg.LanBeacon(LINK_PORT, "phone", addr)
            val bytes = sealedEnv(msg).toJson().encodeToByteArray()
            // Each target on its own: with only a hotspot up the default
            // network is cellular, the limited broadcast fails there,
            // and it must not take the swlan0 directed send down too.
            for (dst in listOf(LIMITED_BCAST) + directedBroadcasts()) {
                runCatching { sock.send(DatagramPacket(bytes, bytes.size, dst)) }
            }
            sendRelay(msg)
            delay(BEACON_MS)
        }
    }

    /** Up, non-loopback IPv4s as (address, hasBroadcast), or `null`
     *  when enumeration fails — callers must treat "don't know" as
     *  "keep the link", never as "the interface is gone". */
    private fun lanIfAddrs(): List<Pair<String, Boolean>>? =
        runCatching {
            java.net.NetworkInterface.getNetworkInterfaces().asSequence()
                .filter { it.isUp && !it.isLoopback }
                .flatMap { it.interfaceAddresses.asSequence() }
                .mapNotNull { ia ->
                    ia.address.hostAddress
                        ?.takeIf { '.' in it && !it.startsWith("169.254.") }
                        ?.let { it to (ia.broadcast != null) }
                }
                .toList()
        }.getOrNull()

    /** The address peers dial us on — prefer a broadcast-capable (LAN)
     *  interface so a cellular/VPN point-to-point address is never
     *  advertised while Wi-Fi is up. */
    private fun lanAddr(): String? =
        lanIfAddrs()?.let { a -> (a.firstOrNull { it.second } ?: a.firstOrNull())?.first }

    /** Called on connectivity changes — if the interface a live link
     *  arrived on went away (Wi-Fi off), the link is already dead;
     *  drop it now instead of waiting for a write error. A failed
     *  enumeration must not kill a healthy link — onLost fires for
     *  unrelated networks (cellular, VPN churn) all the time. */
    fun onNetworkChanged() {
        if (links.isEmpty()) return
        val addrs = lanIfAddrs() ?: return
        for ((peer, l) in links) {
            val local = l.sock.localAddress?.hostAddress ?: continue
            if (addrs.none { it.first == local }) {
                drop(peer, "local addr $local gone")
            }
        }
    }

    /** Subnet-directed broadcast target per up non-loopback interface —
     *  e.g. 192.168.31.255 for wlan0. Java hands us the address directly;
     *  no permissions needed. */
    private fun directedBroadcasts(): List<InetSocketAddress> =
        runCatching {
            java.net.NetworkInterface.getNetworkInterfaces().asSequence()
                .filter { it.isUp && !it.isLoopback }
                .flatMap { it.interfaceAddresses.asSequence() }
                .mapNotNull { it.broadcast?.hostAddress }
                .map { InetSocketAddress(it, BEACON_PORT) }
                .toList()
        }.getOrElse { emptyList() }

    // -- link -----------------------------------------------------------------

    private suspend fun acceptLoop() {
        // Same retry story as the beacon socket — a busy :17894 or a
        // transient bind failure must not disable inbound links forever.
        while (srv == null && started) {
            srv = runCatching { ServerSocket(LINK_PORT) }
                .onFailure { Log.w(TAG, "listen :$LINK_PORT: $it") }.getOrNull()
            if (srv == null) delay(SOCKET_RETRY_MS)
        }
        val server = srv ?: return
        while (started) {
            val s = runCatching { server.accept() }.getOrNull() ?: break
            s.tcpNoDelay = true
            // A dead scope would swallow the socket silently — close it
            // instead of leaving a connected-but-unserved link.
            if (!scope.isActive) {
                runCatching { s.close() }
                break
            }
            scope.launch(Dispatchers.IO) { serveLink(s) }
        }
    }

    /** Dial remembered laptops that aren't linked. The laptop's accept
     *  side waits for a frame that opens, so we say hello first; its
     *  own beacon back is what adopts the socket here. Jittered so a
     *  laptop redialing on the same cadence rarely collides. */
    private suspend fun dialLoop() {
        while (started) {
            delay(DIAL_MS + (0L..DIAL_JITTER_MS).random())
            for ((peer, addr) in peerAddrs) {
                if (links.containsKey(peer) || !started) continue
                val s = runCatching {
                    Socket().apply {
                        connect(InetSocketAddress(addr.hostString, addr.port), DIAL_TIMEOUT_MS)
                        tcpNoDelay = true
                    }
                }.getOrNull() ?: continue
                val hi = sealedEnv(Msg.LanBeacon(LINK_PORT, "phone", lanAddr()))
                val ok = runCatching {
                    s.getOutputStream().apply { write((hi.toJson() + "\n").toByteArray()); flush() }
                }.isSuccess
                if (!ok || links.containsKey(peer)) {
                    runCatching { s.close() }
                    continue
                }
                Log.i(TAG, "dialed ${addr.hostString}:${addr.port} ($peer)")
                scope.launch(Dispatchers.IO) { serveLink(s) }
            }
        }
    }

    private fun serveLink(s: Socket) {
        // The peer's own broadcasts reach the link every few seconds;
        // longer silence = dead transport (Wi-Fi off kills the route
        // without RST, and TCP alone would idle for minutes).
        s.soTimeout = LINK_IDLE_MS
        var peerId: String? = null
        val r = s.getInputStream().bufferedReader()
        while (true) {
            val read = runCatching { readLineBounded(r) }
            val line = read.getOrNull() ?: run {
                Log.i(TAG, "link read ended (${read.exceptionOrNull()?.javaClass?.simpleName}: ${read.exceptionOrNull()?.message})")
                break
            }
            if (line.isEmpty()) continue
            val env = runCatching { Envelope.fromJson(line) }.getOrNull() ?: continue
            if (env.protoMismatch()) {
                Log.w(TAG, "${env.from} speaks proto v${env.v} — we speak v${Envelope.VERSION}; upgrade the peer")
                continue
            }
            val pt = runCatching {
                key.open(android.util.Base64.decode(env.payload, android.util.Base64.NO_WRAP))
            }.getOrNull() ?: continue
            val msg = runCatching {
                protoJson.decodeFromString(Msg.serializer(), pt.decodeToString())
            }.getOrNull() ?: continue
            // Every frame that opens proves family membership → adopt
            // this socket under its sender. A reconnect from the same
            // peer replaces the stale link; different peers coexist.
            if (peerId != env.from) {
                peerId = env.from
                val fresh = !links.containsKey(env.from)
                // A reconnect replaces the stale link — close the old
                // socket so its reader thread exits too.
                links.put(env.from, Link(s, s.getOutputStream()))
                    ?.let { runCatching { it.sock.close() } }
                if (fresh) Log.i(TAG, "lan link up with ${env.from}")
                // Fallback rendezvous until its beacon names the port.
                if (!peerAddrs.containsKey(env.from)) {
                    s.inetAddress?.hostAddress?.let { rememberAddr(env.from, it, LINK_PORT) }
                }
                publishPeers()
            }
            // Link-local keepalives stay in the transport — never reach
            // the service's dispatch/dedup.
            when (msg) {
                is Msg.LanPing -> {
                    val pong = sealedEnv(Msg.LanPong(msg.ts))
                    hiQ.trySend(Write(env.from, (pong.toJson() + "\n").toByteArray()))
                    continue
                }
                is Msg.LanPong -> {
                    links[env.from]?.let {
                        it.rttMs = (System.currentTimeMillis() - msg.ts).coerceAtLeast(0)
                        publishPeers()
                    }
                    continue
                }
                is Msg.LanBeacon -> {
                    // A laptop's in-link beacon names where it accepts.
                    if (msg.kind != "phone") {
                        msg.lan_addr?.let { rememberAddr(env.from, it, msg.tcp_port) }
                    }
                }
                else -> {}
            }
            onMsg(env)
        }
        peerId?.let { if (links[it]?.sock === s) drop(it, "read ended") }
        runCatching { s.close() }
    }

    /** Links have no handshake — an unauthenticated LAN peer could feed
     *  newline-free garbage forever and grow the reader's buffer without
     *  bound. `readLine` can't cap itself, so we read chars manually:
     *  null at EOF, the line at '\n', [OversizeFrame] past the wire cap. */
    private class OversizeFrame : Exception("frame over wire cap")

    private fun readLineBounded(r: java.io.BufferedReader): String? {
        val sb = StringBuilder()
        while (true) {
            when (val c = r.read()) {
                -1 -> return if (sb.isEmpty()) null else sb.toString()
                '\n'.code -> return sb.toString()
                else -> {
                    if (sb.length >= Envelope.MAX_ENVELOPE_JSON) throw OversizeFrame()
                    sb.append(c.toChar())
                }
            }
        }
    }

    private companion object {
        const val TAG = "LanLink"
        const val BEACON_PORT = 17893
        const val LINK_PORT = 17894
        const val BEACON_MS = 4000L
        const val PING_MS = 5000L
        const val LINK_IDLE_MS = 20_000
        const val SOCKET_RETRY_MS = 2_000L
        const val DIAL_MS = 10_000L
        const val DIAL_JITTER_MS = 3_000L
        const val DIAL_TIMEOUT_MS = 3_000
        const val KEY_ADDRS = "peer_addrs"
        val LIMITED_BCAST = InetSocketAddress("255.255.255.255", BEACON_PORT)
    }
}
