package dev.murmur

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.Network
import android.os.IBinder
import android.util.Base64
import dev.murmur.bridge.MediaBridge
import dev.murmur.core.Envelope
import dev.murmur.core.FeatureHub
import dev.murmur.core.PresenceTable
import dev.murmur.core.FamilyKey
import dev.murmur.core.LanLink
import dev.murmur.core.Msg
import dev.murmur.core.Relay
import dev.murmur.core.Control
import dev.murmur.core.DeviceInfo
import dev.murmur.core.Secrets
import dev.murmur.core.protoJson
import dev.murmur.feature.ClipboardFeature
import dev.murmur.feature.KeyringFeature
import dev.murmur.feature.MediaFeature
import dev.murmur.feature.NotifyFeature
import dev.murmur.feature.PresenceFeature
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.serialization.encodeToString

/**
 * The phone-side agent. Owns the relay, the family key and the bridges;
 * everything else is a one-line dispatch.
 *
 * The phone keeps a presence table (device → (state, ts)) and elects the
 * active laptop exactly like murmurd does — needed to know which laptop's
 * clipboard to apply and which device pair-frames come from. The phone
 * itself never broadcasts Presence: it's not a laptop-election candidate.
 */
class MurmurService : Service() {
    // capabilities announced to the family — mirrors the bridges this build ships


    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private lateinit var secrets: Secrets
    private lateinit var key: FamilyKey
    private lateinit var relay: Relay
    private lateinit var lan: LanLink
    private lateinit var media: MediaBridge
    private lateinit var identity: dev.murmur.core.IdentityKey

    /** Laptop presence + the active-device election — fed by the
     *  presence feature, read by the clipboard feature's apply rule. */
    private val presenceTable = PresenceTable()
    private val hub = FeatureHub()

    /** Recently seen (sender, seq) — frames arrive over both the LAN
     *  early-copy path and the relay; the duplicate collapses here.
     *  [lastSeq] keeps the check correct across a peer restart: its
     *  seq resets to 1, and a frame far below the high-water mark is
     *  fresh, not a duplicate — so the sender's window resets. */
    private val seenSeq = LinkedHashSet<Pair<String, Long>>()
    private val lastSeq = HashMap<String, Long>()

    /** ElapsedRealtimeNanos of the in-flight ListDevices — its reply RTT
     *  is the "ping" the dashboard shows. */
    private val listSentAt = java.util.concurrent.atomic.AtomicLong()

    /** True in a serverless family — the relay object never exists. */
    private var serverless = false

    /** Envelope seq for a family with no relay counter. */
    private val notifLock = Any()
    @Volatile private var destroyed = false
    /** Stopped by the shade's "pause" (not exit / tile / dashboard). */
    @Volatile private var pausedFromShade = false

    private val localSeq = java.util.concurrent.atomic.AtomicLong(System.currentTimeMillis())

    /** The family card in a serverless world: roster members + self,
     *  online = currently holding a live LAN link. Names arrive with
     *  roster gossip; until then a peer shows as its device id. */
    private fun localDevices(peers: Map<String, Long>): List<DeviceInfo> {
        val ros = secrets.roster
        val names = secrets.peerNames
        val selfId = secrets.deviceId
        val rows = ros.keys.map { id ->
            DeviceInfo(
                device_id = id,
                name = names[id] ?: id,
                revoked = false,
                online = peers.containsKey(id),
                rtt_ms = peers[id],
            )
        }
        return rows + DeviceInfo(
            device_id = selfId ?: "",
            name = secrets.deviceName,
            revoked = false,
            online = true,
            rtt_ms = null,
        )
    }

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            if (::relay.isInitialized) relay.kick()
        }
        override fun onLost(network: Network) {
            // Wi-Fi leaving drops the LAN link's interface — kill the
            // link now instead of letting a zombie linger until the
            // next write error (the badge would lie meanwhile).
            if (::lan.isInitialized) lan.onNetworkChanged()
        }
    }

    override fun onCreate() {
        super.onCreate()
        instance = this
        // startForegroundService() owes a call within 5s even on the
        // early-return path — promote first, decide later.
        startForeground(NOTIF_ID, statusNotification("starting…"))
        getSystemService(NotificationManager::class.java).cancel(PAUSED_ID)
        secrets = Secrets(this)
        key = secrets.familyKey()?.takeIf { !secrets.removed } ?: run {
            // Not provisioned — sit idle; the service is (re)started after
            // pairing completes.
            stopSelf()
            return
        }

        // A serverless family has no relay in its world — the whole
        // transport is the LAN mesh, and nothing below may touch a
        // websocket.
        serverless = secrets.familyKind == "serverless"
        if (!serverless) {
            relay = Relay(
                url = secrets.serverUrl,
                hello = {
                    dev.murmur.core.Control.Hello(
                        device_id = secrets.deviceId!!,
                        name = secrets.deviceName,
                        role = "auth",
                        token_hex = secrets.authTokenHex(),
                        proto_v = Envelope.VERSION,
                    )
                },
                scope = scope,
                onAuthed = {
                    announceIdentity()
                    media.publishNow()
                    pollDevices()
                },
            )
        }
        identity = secrets.identity()
        media = MediaBridge(this, ::send, scope)
        lan = LanLink(
            secrets.deviceId!!, key, scope, { onEnvelope(it) },
            prefs = getSharedPreferences("lan", MODE_PRIVATE),
        ) { send(it) }
        status.value = Status(
            provisioned = true,
            running = true,
            familyKind = secrets.familyKind,
            // Serverless never gets a Devices reply — the name stored
            // at pairing is the only source until gossip lands.
            familyName = secrets.familyName,
            startedAtMs = android.os.SystemClock.elapsedRealtime(),
        )
        // Feature hub — mirrors murmurd's feature.rs: adding a feature
        // never touches the pipeline below, it wires in here.
        hub.wire(PresenceFeature(presenceTable))
        hub.wire(KeyringFeature(
            secrets, identity, presenceTable, secrets.deviceId!!,
            send = { msg, to -> send(msg, to) },
            onKeyRotated = { newKey, epoch ->
                key = newKey
                status.update { it.copy(keyEpoch = epoch) }
            },
            onKicked = {
                // Same shape as the relay's REVOKED: keys off the wire,
                // dashboard offers re-pairing.
                secrets.removed = true
                stopSelf()
            },
        ))
        hub.wire(ClipboardFeature(this, secrets) { activeDevice() })
        hub.wire(MediaFeature(media))
        hub.wire(NotifyFeature())
        if (!serverless) relay.start()
        lan.start()
        // Shares queued while the service was down go out on first
        // transport readiness — the relay for serverful, any live link
        // for serverless.
        scope.launch {
            status.first { it.relay == Relay.State.ONLINE || it.lanPeers.isNotEmpty() }
            dev.murmur.core.PendingPush.drain(this@MurmurService).forEach {
                android.util.Log.i("MurmurService", "delivering queued share (${it.length}B)")
                pushClipboard(it)
            }
        }
        var seenPeers = emptySet<String>()
        scope.launch {
            lan.peersFlow.collect { p ->
                // Serverless has no ListDevices — the family card is
                // synthesised locally from the roster + live links
                // until roster gossip lands.
                status.update {
                    val st = it.copy(lanPeers = p)
                    if (serverless) st.copy(devices = localDevices(p)) else st
                }
                if (serverless) {
                    // No relay connect event exists — a link-up IS the
                    // rendezvous. Announce so the peer pins our identity
                    // and name; fresh media state follows so its
                    // dashboard isn't blank.
                    val arrived = p.keys - seenPeers
                    if (arrived.isNotEmpty()) {
                        arrived.forEach { announceIdentity(to = it) }
                        media.publishNow()
                    }
                }
                seenPeers = p.keys
            }
        }
        scope.launch {
            lan.portBusyFlow.collect { busy ->
                status.update { it.copy(portBusy = busy) }
            }
        }
        scope.launch {
            lan.lanAddrFlow.collect { a ->
                status.update { it.copy(lanAddr = a) }
            }
        }
        if (!serverless) {
            scope.launch { relay.incoming.collect { onEnvelope(it) } }
            scope.launch {
                relay.state.collect { st ->
                    status.update { it.copy(relay = st) }
                    if (st == Relay.State.ONLINE) {
                        // Cheap self-heal: the family list always shows the
                        // current name — debug installs suffix it with ·dev.
                        pushName()
                    }
                    if (st == Relay.State.REVOKED) {
                        // The relay forgot us — nothing to retry. Keep the keys
                        // off the wire and let the dashboard offer re-pairing.
                        secrets.removed = true
                        stopSelf()
                    }
                }
            }
        }
        // The shade notification mirrors the link, so the phone's state is
        // readable without opening the app.
        scope.launch {
            status.collect { st ->
                val laptops = st.devices.count {
                    it.online && !it.revoked && it.device_id != secrets.deviceId
                }
                val text = if (serverless) {
                    when (st.lanPeers.size) {
                        0 -> "offline — no peers"
                        else -> "online · ${st.lanPeers.size} peer(s) · lan only"
                    }
                } else when (st.relay) {
                    Relay.State.ONLINE -> when (laptops) {
                        0 -> "online · no laptop around"
                        1 -> "online · 1 device"
                        else -> "online · $laptops devices"
                    }
                    Relay.State.CONNECTING -> "connecting…"
                    Relay.State.OFFLINE -> "offline — retrying"
                    Relay.State.REVOKED -> "removed from the family"
                }
                synchronized(notifLock) {
                    // A post racing onDestroy would orphan a stale
                    // notification once the service is gone.
                    if (!destroyed) {
                        getSystemService(NotificationManager::class.java)
                            .notify(NOTIF_ID, statusNotification(text))
                    }
                }
            }
        }

        if (!serverless) {
            // Push our display name (idempotent server-side upsert) and keep
            // the family list + server ping fresh while online.
            scope.launch {
                relay.state.collect { st ->
                    if (st == Relay.State.ONLINE) pushName()
                }
            }
            // Device list + ping only matter while someone looks at them —
            // poll while the dashboard is visible, stay silent otherwise.
            scope.launch {
                combine(uiVisible, relay.state) { v, st -> v && st == Relay.State.ONLINE }
                    .collectLatest { live ->
                        while (live) {
                            pollDevices()
                            kotlinx.coroutines.delay(DEVICES_POLL_MS)
                        }
                    }
            }
        }

        getSystemService(ConnectivityManager::class.java)
            .registerDefaultNetworkCallback(networkCallback)
    }

    override fun onDestroy() {
        synchronized(notifLock) { destroyed = true }
        scope.cancel()
        stopForeground(STOP_FOREGROUND_REMOVE)
        getSystemService(NotificationManager::class.java).run {
            cancel(NOTIF_ID)
            // Paused from the shade: leave a way back there too.
            // Its own id: the system takes the service's id down after
            // onDestroy and would swallow a repost under it.
            if (pausedFromShade) notify(PAUSED_ID, pausedNotification())
        }
        // Unprovisioned starts stopSelf() before relay/secrets exist.
        status.value = Status(
            provisioned = ::secrets.isInitialized && secrets.provisioned,
            running = false,
        )
        if (::relay.isInitialized) relay.stop()
        // Frees :17894 — without it a same-process service restart
        // leaves a zombie listener that swallows laptop dials.
        if (::lan.isInitialized) lan.stop()
        runCatching {
            getSystemService(ConnectivityManager::class.java)
                .unregisterNetworkCallback(networkCallback)
        }
        instance = null
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_PAUSE -> {
                pausedFromShade = true
                stopSelf()
            }
            ACTION_EXIT -> stopSelf()
        }
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun pollDevices() {
        val id = secrets.deviceId ?: return
        listSentAt.set(android.os.SystemClock.elapsedRealtimeNanos())
        relay.sendControl(Control.ListDevices, id)
    }

    private fun announceIdentity(to: String = Envelope.ALL) =
        send(Msg.Identity(identity.publicB64, "phone", secrets.deviceName, KeyringFeature.PHONE_CAPS), to)

    /** Push the locally edited name to the registry. */
    fun pushName() {
        if (!::secrets.isInitialized) return
        val id = secrets.deviceId ?: return
        if (::relay.isInitialized) {
            relay.sendControl(Control.RenameDevice(id, secrets.deviceName), id)
        } else {
            // Serverless has no device registry — the name rides
            // Identity gossip, so re-announce or peers show the old
            // one until a re-link.
            announceIdentity()
        }
    }

    /** Ask the server to kick us out of the family — best effort before
     *  the local wipe on logout. */
    fun revokeSelf() {
        if (!::relay.isInitialized || !::secrets.isInitialized) return
        val id = secrets.deviceId ?: return
        relay.sendControl(Control.RevokeDevice(id), id)
    }

    /** Seal and enqueue an E2E message. */
    fun send(msg: Msg, to: String = Envelope.ALL) {
        val pt = protoJson.encodeToString(Msg.serializer(), msg).encodeToByteArray()
        if (pt.size > Envelope.MAX_PAYLOAD) {
            android.util.Log.w("MurmurService",
                "message over wire cap (${pt.size}B > ${Envelope.MAX_PAYLOAD}B) — dropped")
            return
        }
        val env = Envelope(
            v = Envelope.VERSION,
            from = secrets.deviceId!!,
            to = to,
            seq = if (::relay.isInitialized) relay.nextSeq() else localSeq.getAndIncrement(),
            payload = Base64.encodeToString(key.seal(pt), Base64.NO_WRAP),
        )
        // LAN delivers an early copy when the route matches the link;
        // the relay copy is authoritative and gets deduped on receipt —
        // a silently dead link can never eat a frame. A serverless
        // family has no relay at all — links are the whole transport.
        lan.sendIfPeer(env.toJson(), to)
        if (::relay.isInitialized) relay.sendEnv(env)
    }

    /**
     * Explicit user push (share / process-text): `explicit=true`, every
     * laptop sees it, only the active one applies.
     */
    fun pushClipboard(text: String) {
        android.util.Log.d("MurmurService", "pushClipboard len=${text.length} enabled=${secrets.clipEnabled} send=${secrets.clipSend}")
        if (!secrets.clipEnabled || !secrets.clipSend) return
        send(
            Msg.Clipboard(
                origin = secrets.deviceId!!,
                hash = fnv1a(text.encodeToByteArray()).toULong(),
                mime = "text/plain",
                data_b64 = Base64.encodeToString(text.encodeToByteArray(), Base64.NO_WRAP),
                explicit = true,
                ts = System.currentTimeMillis() / 1000,
            ),
        )
    }

    /** Active laptop by the family election — delegates to the
     *  presence table; see PresenceTable.activeDevice for the rule. */
    fun activeDevice(): String? = presenceTable.activeDevice()

    private fun onEnvelope(env: Envelope) {
        // Cleartext control replies (Devices) — handled before decrypt.
        env.controlPayload()?.let { ctl ->
            if (ctl is Control.Devices) {
                val sent = listSentAt.getAndSet(0)
                status.update { it.copy(
                    devices = ctl.devices,
                    familyName = ctl.family_name,
                    serverRttMs = if (sent != 0L)
                        (android.os.SystemClock.elapsedRealtimeNanos() - sent) / 1_000_000
                    else null,
                ) }
            }
            return
        }
        if (env.to == Envelope.SERVER) return
        val pt = runCatching { key.open(Base64.decode(env.payload, Base64.NO_WRAP)) }.getOrNull()
            ?: return // pairing cleartext or a different family key
        val msg = runCatching {
            protoJson.decodeFromString(Msg.serializer(), pt.decodeToString())
        }.getOrNull() ?: return

        // Broadcasts intentionally ride both LAN (early copy) and the
        // relay — the second copy collapses here. Dedupe only after a
        // successful open so forged frames can't poison the table.
        synchronized(seenSeq) {
            val last = lastSeq[env.from] ?: 0L
            if (env.seq <= last && last - env.seq > RESTART_GAP) {
                // seq jumped far backwards — the peer restarted; its
                // stored entries predate the restart and would swallow
                // fresh frames.
                seenSeq.removeIf { it.first == env.from }
                lastSeq[env.from] = env.seq
            } else if (env.seq > last) {
                lastSeq[env.from] = env.seq
            }
            if (!seenSeq.add(env.from to env.seq)) return
            if (seenSeq.size > SEEN_MAX) seenSeq.remove(seenSeq.first())
        }

        // Tag-dispatch to the wired features — same invariant as
        // murmurd: dedupe happened above, features see a frame once.
        hub.dispatch(env.from, msg)
    }

    private fun statusNotification(text: String): Notification {
        val mgr = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        mgr.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "murmur", NotificationManager.IMPORTANCE_MIN),
        )
        val open = android.app.PendingIntent.getActivity(
            this, 0,
            Intent(this, dev.murmur.ui.MainActivity::class.java),
            android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        val pause = android.app.PendingIntent.getService(
            this, 1,
            Intent(this, MurmurService::class.java).setAction(ACTION_PAUSE),
            android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        val exit = android.app.PendingIntent.getService(
            this, 3,
            Intent(this, MurmurService::class.java).setAction(ACTION_EXIT),
            android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("murmur")
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "pause", pause).build())
            .addAction(Notification.Action.Builder(null, "exit", exit).build())
            .setOngoing(true)
            .build()
    }

    /** Left behind by the shade's "pause": resume or exit from the same
     *  place. Not a service notification — swiping it away is exit. */
    private fun pausedNotification(): Notification {
        val open = android.app.PendingIntent.getActivity(
            this, 0,
            Intent(this, dev.murmur.ui.MainActivity::class.java),
            android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        val resume = android.app.PendingIntent.getForegroundService(
            this, 2,
            Intent(this, MurmurService::class.java),
            android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        val exit = android.app.PendingIntent.getBroadcast(
            this, 4,
            Intent(this, ExitReceiver::class.java),
            android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("murmur")
            .setContentText("paused")
            .setSmallIcon(R.drawable.ic_stat)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "resume", resume).build())
            .addAction(Notification.Action.Builder(null, "exit", exit).build())
            .build()
    }

    data class Status(
        val relay: Relay.State = Relay.State.OFFLINE,
        val provisioned: Boolean = false,
        /** Service process is alive — the QS tile + pause button's truth. */
        val running: Boolean = false,
        /** ListDevices round-trip — the phone's own ping to the relay. */
        val serverRttMs: Long? = null,
        /** Peer id → measured LAN round-trip in ms (0 = linked but not
         *  yet measured). Several laptops may each hold a link — the
         *  UI shows "on lan" per device row with the real ping. */
        val lanPeers: Map<String, Long> = emptyMap(),
        /** This phone's LAN address — what a laptop `murmurctl dial`s
         *  when broadcast discovery can't pass. */
        val lanAddr: String? = null,
        /** When the service came up — the UI's "connecting" grace. */
        val startedAtMs: Long = 0L,
        /** Family kind from secrets — "serverful" | "serverless". */
        val familyKind: String = "serverful",
        val devices: List<DeviceInfo> = emptyList(),
        val familyName: String? = null,
        val keyEpoch: Long = 0,
        /** Listen port stayed busy across retries — another copy of
         *  murmur is almost certainly installed on this phone. */
        val portBusy: Boolean = false,
    )

    companion object {
        private const val CHANNEL_ID = "murmur.relay"
        private const val NOTIF_ID = 1
        internal const val PAUSED_ID = 2
        private const val DEVICES_POLL_MS = 5_000L
        private const val ACTION_PAUSE = "dev.murmur.PAUSE"
        private const val ACTION_EXIT = "dev.murmur.EXIT"
        private const val SEEN_MAX = 4096
        /** This far below the sender's high-water seq can't be an
         *  in-flight duplicate — the LAN early copy is never that late;
         *  it's a peer restart (seq resets to 1). */
        private const val RESTART_GAP = 64L

        /** The dashboard is on screen — gates the device-list poller. */
        val uiVisible = MutableStateFlow(false)

        var instance: MurmurService? = null
            private set

        /** Live status for the UI layer. */
        val status = MutableStateFlow(Status())

        fun start(ctx: Context) {
            ctx.startForegroundService(Intent(ctx, MurmurService::class.java))
        }

        fun stop(ctx: Context) {
            ctx.stopService(Intent(ctx, MurmurService::class.java))
        }

        /** FNV-1a — identical to murmurd::clipboard::hash_clip. */
        fun fnv1a(data: ByteArray): Long {
            var h = 0xcbf29ce484222325uL.toLong()
            for (b in data) {
                // Byte.toLong() sign-extends — Rust uses u64::from(u8)
                // (zero-extend). Mask first or high bytes hash differently.
                h = h xor (b.toLong() and 0xFF)
                h *= 0x100000001b3uL.toLong()
            }
            return h
        }
    }
}
