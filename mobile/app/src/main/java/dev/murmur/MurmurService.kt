package dev.murmur

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
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
import kotlinx.coroutines.launch
import kotlinx.serialization.encodeToString

/**
 * The phone-side agent. Owns the relay, the family key and the bridges;
 * everything else is a one-line dispatch.
 *
 * Bridges reach the service through [instance] (system services like the
 * NotificationListener can't use binders across their lifecycle quirks).
 */
class MurmurService : Service() {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private lateinit var secrets: Secrets
    private lateinit var key: FamilyKey
    private lateinit var relay: Relay
    private lateinit var media: MediaBridge

    override fun onCreate() {
        super.onCreate()
        instance = this
        secrets = Secrets(this)
        key = secrets.familyKey() ?: run {
            // Not provisioned yet — sit idle until pairing completes.
            // TODO(pairing): start in role=pair and run the handshake.
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
        relay.start()
        scope.launch {
            relay.incoming.collect { env -> onEnvelope(env) }
        }

        startForeground(NOTIF_ID, statusNotification("online"))
    }

    override fun onDestroy() {
        relay.stop()
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

    /** Explicit user push (share / process-text) — goes to all laptops;
     * only the active one applies it. */
    fun pushClipboard(text: String) {
        send(
            Msg.Clipboard(
                origin = secrets.deviceId!!,
                hash = fnv1a(text.encodeToByteArray()),
                mime = "text/plain",
                data_b64 = Base64.encodeToString(text.encodeToByteArray(), Base64.NO_WRAP),
                ts = System.currentTimeMillis() / 1000,
            ),
        )
    }

    private fun onEnvelope(env: Envelope) {
        if (env.to == Envelope.SERVER) return
        val pt = runCatching { key.open(Base64.decode(env.payload, Base64.NO_WRAP)) }.getOrNull()
            ?: return // not for our key / pairing cleartext
        val msg = runCatching { protoJson.decodeFromString(Msg.serializer(), pt.decodeToString()) }.getOrNull()
            ?: return
        when (msg) {
            is Msg.MediaCommand -> media.execute(msg)
            is Msg.Clipboard -> ClipboardBridge.apply(this, msg)
            is Msg.NotificationDismiss -> NotificationBridge.requestCancel(msg.key)
            is Msg.KeyRotate -> {
                // TODO: persist new key atomically, then re-key relay
            }
            is Msg.Presence -> Unit // election lives on the laptops
            else -> Unit
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
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setOngoing(true)
            .build()
    }

    companion object {
        private const val CHANNEL_ID = "murmur.relay"
        private const val NOTIF_ID = 1

        var instance: MurmurService? = null
            private set

        fun start(ctx: Context) {
            ctx.startForegroundService(Intent(ctx, MurmurService::class.java))
        }

        /** FNV-1a, same as murmurd::clipboard::hash_clip. */
        fun fnv1a(data: ByteArray): Long {
            var h = 0xcbf29ce484222325uL.toLong()
            for (b in data) {
                h = h xor b.toLong()
                h *= 0x100000001b3
            }
            return h
        }
    }
}
