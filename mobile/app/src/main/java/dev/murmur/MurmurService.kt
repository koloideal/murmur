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
import dev.murmur.core.Msg
import dev.murmur.core.Relay
import dev.murmur.core.Secrets
import dev.murmur.core.protoJson
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
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
    private lateinit var media: MediaBridge

    /** device → (state, ts), mirrored from Presence broadcasts. */
    private val presence = mutableMapOf<String, Pair<String, Long>>()

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
        startForeground(NOTIF_ID, statusNotification("murmur"))
        secrets = Secrets(this)
        key = secrets.familyKey() ?: run {
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
                    name = android.os.Build.MODEL,
                    role = "auth",
                    token_hex = secrets.authTokenHex(),
                )
            },
            scope = scope,
            onAuthed = { media.publishNow() },
        )
        media = MediaBridge(this, ::send, scope)
        status.value = Status(provisioned = true)
        relay.start()
        scope.launch { relay.incoming.collect { onEnvelope(it) } }
        scope.launch { relay.state.collect { status.value = status.value.copy(relay = it) } }

        getSystemService(ConnectivityManager::class.java)
            .registerDefaultNetworkCallback(networkCallback)
    }

    override fun onDestroy() {
        relay.stop()
        runCatching {
            getSystemService(ConnectivityManager::class.java)
                .unregisterNetworkCallback(networkCallback)
        }
        scope.cancel()
        instance = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /** Seal and enqueue an E2E message. */
    fun send(msg: Msg, to: String = Envelope.ALL) {
        val pt = protoJson.encodeToString(Msg.serializer(), msg).encodeToByteArray()
        relay.send(to, Base64.encodeToString(key.seal(pt), Base64.NO_WRAP), secrets.deviceId!!)
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
            is Msg.KeyRotate -> {
                val newKey = runCatching {
                    Base64.decode(msg.new_key_b64, Base64.NO_WRAP)
                }.getOrNull() ?: return
                secrets.rotateFamilyKey(newKey)
                key = FamilyKey.fromBytes(newKey)
            }
            else -> Unit // Notification/MediaState are phone-originated
        }
    }

    private fun statusNotification(text: String): Notification {
        val mgr = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        mgr.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "murmur", NotificationManager.IMPORTANCE_MIN),
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("murmur")
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_stat)
            .setOngoing(true)
            .build()
    }

    data class Status(
        val relay: Relay.State = Relay.State.OFFLINE,
        val provisioned: Boolean = false,
    )

    companion object {
        private const val CHANNEL_ID = "murmur.relay"
        private const val NOTIF_ID = 1

        var instance: MurmurService? = null
            private set

        /** Live status for the UI layer. */
        val status = MutableStateFlow(Status())

        fun start(ctx: Context) {
            ctx.startForegroundService(Intent(ctx, MurmurService::class.java))
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
