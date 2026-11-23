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
import dev.murmur.bridge.ClipboardBridge
import dev.murmur.bridge.MediaBridge
import dev.murmur.bridge.NotificationBridge
import dev.murmur.core.Envelope
import dev.murmur.core.FamilyKey
import dev.murmur.core.LanLink
import dev.murmur.core.Msg
import dev.murmur.core.Relay
import dev.murmur.core.Control
import dev.murmur.core.DeviceInfo
import dev.murmur.core.Secrets
import dev.murmur.core.protoJson
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
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

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private lateinit var secrets: Secrets
    private lateinit var key: FamilyKey
    private lateinit var relay: Relay
    private lateinit var lan: LanLink
    private lateinit var media: MediaBridge
    private lateinit var identity: dev.murmur.core.IdentityKey

    /** device → (state, ts), mirrored from Presence broadcasts. */
    private val presence = mutableMapOf<String, Pair<String, Long>>()

    /** ElapsedRealtimeNanos of the in-flight ListDevices — its reply RTT
     *  is the "ping" the dashboard shows. */
    private val listSentAt = java.util.concurrent.atomic.AtomicLong()

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            if (::relay.isInitialized) relay.kick()
        }
    }

    override fun onCreate() {
        super.onCreate()
        instance = this
        // startForegroundService() owes a call within 5s even on the
        // early-return path — promote first, decide later.
        startForeground(NOTIF_ID, statusNotification("starting…"))
        secrets = Secrets(this)
        key = secrets.familyKey()?.takeIf { !secrets.removed } ?: run {
            // Not provisioned — sit idle; the service is (re)started after
            // pairing completes.
            stopSelf()
            return
        }

        relay = Relay(
            url = secrets.serverUrl,
            hello = {
                dev.murmur.core.Control.Hello(
                    device_id = secrets.deviceId!!,
                    name = secrets.deviceName,
                    role = "auth",
                    token_hex = secrets.authTokenHex(),
                )
            },
            scope = scope,
            onAuthed = {
                announceIdentity()
                media.publishNow()
                pollDevices()
            },
        )
        identity = secrets.identity()
        media = MediaBridge(this, ::send, scope)
        lan = LanLink(secrets.deviceId!!, key, scope) { onEnvelope(it) }
        status.value = Status(provisioned = true, running = true)
        relay.start()
        lan.start()
        scope.launch { relay.incoming.collect { onEnvelope(it) } }
        scope.launch {
            relay.state.collect { st ->
                status.value = status.value.copy(relay = st)
                if (st == Relay.State.REVOKED) {
                    // The relay forgot us — nothing to retry. Keep the keys
                    // off the wire and let the dashboard offer re-pairing.
                    secrets.removed = true
                    stopSelf()
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
                val text = when (st.relay) {
                    Relay.State.ONLINE -> when (laptops) {
                        0 -> "online · no laptop around"
                        1 -> "online · 1 device"
                        else -> "online · $laptops devices"
                    }
                    Relay.State.CONNECTING -> "connecting…"
                    Relay.State.OFFLINE -> "offline — retrying"
                    Relay.State.REVOKED -> "removed from the family"
                }
                getSystemService(NotificationManager::class.java)
                    .notify(NOTIF_ID, statusNotification(text))
            }
        }

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

        getSystemService(ConnectivityManager::class.java)
            .registerDefaultNetworkCallback(networkCallback)
    }

    override fun onDestroy() {
        // Unprovisioned starts stopSelf() before relay/secrets exist.
        status.value = Status(
            provisioned = ::secrets.isInitialized && secrets.provisioned,
            running = false,
        )
        if (::relay.isInitialized) relay.stop()
        runCatching {
            getSystemService(ConnectivityManager::class.java)
                .unregisterNetworkCallback(networkCallback)
        }
        scope.cancel()
        instance = null
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_PAUSE) stopSelf()
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun pollDevices() {
        val id = secrets.deviceId ?: return
        listSentAt.set(android.os.SystemClock.elapsedRealtimeNanos())
        relay.sendControl(Control.ListDevices, id)
    }

    private fun announceIdentity(to: String = Envelope.ALL) =
        send(Msg.Identity(identity.publicB64, "phone"), to)

    /** TOFU: the first identity a member announces under the current
     *  family key is the one we trust for its key rotations. */
    private fun pin(from: String, id: Msg.Identity) {
        if (from == secrets.deviceId || from in secrets.revokedIds) return
        val roster = secrets.roster
        if (roster.containsKey(from)) return
        secrets.roster = roster + (from to (id.pub_b64 to id.kind))
        announceIdentity(to = from) // they may have missed ours
    }

    /** Adopt a rotation sealed to our identity by a pinned laptop.
     *  Newer epoch wins; same epoch → smaller rotator id (two laptops
     *  rotating at once still converge). */
    private fun applyRotation(from: String, r: Msg.KeyRotate) {
        val cur = secrets.keyEpoch
        val rot = secrets.keyRotator
        val newer = r.epoch > cur || (r.epoch == cur && rot != null && from < rot)
        if (!newer) return
        val rotatorPub = secrets.roster[from]?.first ?: return
        val mine = r.keys.firstOrNull { it.device_id == secrets.deviceId } ?: return
        val newKey = identity.openRekey(rotatorPub, r.epoch, mine.sealed_b64) ?: return
        secrets.rotateFamilyKey(newKey, r.epoch, from, r.revoked)
        key = FamilyKey.fromBytes(newKey)
        r.revoked.forEach { presence.remove(it) }
        status.value = status.value.copy(keyEpoch = r.epoch)
        announceIdentity()
    }

    /** Push the locally edited name to the registry. */
    fun pushName() {
        if (!::relay.isInitialized || !::secrets.isInitialized) return
        val id = secrets.deviceId ?: return
        relay.sendControl(Control.RenameDevice(id, secrets.deviceName), id)
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
        val env = Envelope(
            v = Envelope.VERSION,
            from = secrets.deviceId!!,
            to = to,
            seq = relay.nextSeq(),
            payload = Base64.encodeToString(key.seal(pt), Base64.NO_WRAP),
        )
        // LAN unicast to the linked peer skips the relay entirely;
        // broadcasts still ride the relay — the link just delivers an
        // early copy to whoever is linked.
        if (!lan.sendIfPeer(env.toJson(), to) || to == Envelope.ALL) {
            relay.sendEnv(env)
        }
    }

    /**
     * Explicit user push (share / process-text): `explicit=true`, every
     * laptop sees it, only the active one applies.
     */
    fun pushClipboard(text: String) {
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

    /** Active laptop = newest `active` presence; ties → smaller id. The
     *  same deterministic rule murmurd uses, computed over laptop ids only
     *  (the phone never reports presence). */
    fun activeDevice(): String? = presence
        .filterValues { (state, _) -> state == "active" }
        .maxWithOrNull(
            compareBy<Map.Entry<String, Pair<String, Long>>> { it.value.second }
                .thenByDescending { it.key },
        )
        ?.key

    private fun onEnvelope(env: Envelope) {
        // Cleartext control replies (Devices) — handled before decrypt.
        env.controlPayload()?.let { ctl ->
            if (ctl is Control.Devices) {
                val sent = listSentAt.getAndSet(0)
                status.value = status.value.copy(
                    devices = ctl.devices,
                    familyName = ctl.family_name,
                    serverRttMs = if (sent != 0L)
                        (android.os.SystemClock.elapsedRealtimeNanos() - sent) / 1_000_000
                    else null,
                )
            }
            return
        }
        if (env.to == Envelope.SERVER) return
        val pt = runCatching { key.open(Base64.decode(env.payload, Base64.NO_WRAP)) }.getOrNull()
            ?: return // pairing cleartext or a different family key
        val msg = runCatching {
            protoJson.decodeFromString(Msg.serializer(), pt.decodeToString())
        }.getOrNull() ?: return

        when (msg) {
            is Msg.Presence -> presence[env.from] = msg.state to msg.ts
            is Msg.MediaCommand -> media.execute(msg)
            is Msg.Clipboard -> {
                // Phone-side rule: apply only clips from the active laptop.
                if (msg.origin == activeDevice()) ClipboardBridge.apply(this, msg)
            }
            is Msg.NotificationDismiss -> NotificationBridge.requestCancel(msg.key)
            is Msg.Identity -> pin(env.from, msg)
            is Msg.KeyRotate -> applyRotation(env.from, msg)
            else -> Unit // Notification/MediaState are phone-originated
        }
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
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("murmur")
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "pause", pause).build())
            .setOngoing(true)
            .build()
    }

    data class Status(
        val relay: Relay.State = Relay.State.OFFLINE,
        val provisioned: Boolean = false,
        /** Service process is alive — the QS tile + pause button's truth. */
        val running: Boolean = false,
        /** ListDevices round-trip — the phone's own ping to the relay. */
        val serverRttMs: Long? = null,
        val devices: List<DeviceInfo> = emptyList(),
        val familyName: String? = null,
        val keyEpoch: Long = 0,
    )

    companion object {
        private const val CHANNEL_ID = "murmur.relay"
        private const val NOTIF_ID = 1
        private const val DEVICES_POLL_MS = 12_000L
        private const val ACTION_PAUSE = "dev.murmur.PAUSE"

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
