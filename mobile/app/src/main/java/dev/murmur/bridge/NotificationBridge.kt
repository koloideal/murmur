package dev.murmur.bridge

import android.app.Notification
import android.app.Notification.MessagingStyle
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.drawable.Icon
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Base64
import android.util.Log
import dev.murmur.MurmurService
import dev.murmur.core.Msg
import dev.murmur.core.Secrets
import kotlinx.coroutines.flow.MutableStateFlow
import java.io.ByteArrayOutputStream
import java.util.concurrent.ConcurrentHashMap

/**
 * System-bound notification listener — the source of truth for what the
 * phone shows. Posted → broadcast; removed → dismiss everywhere, unless we
 * cancelled it ourselves on a remote dismiss (anti-loop via [remoteCancels]).
 *
 * Privacy gates, in order: ongoing/group-summary noise, MediaStyle
 * track-churn (the laptop mirrors playback itself), muted apps,
 * `VISIBILITY_SECRET` (the app itself says "not even on a lock screen"),
 * then the optional "hide message text" mode. Android 15+ additionally
 * redacts OTPs for listeners like us before we ever see them.
 */
class NotificationBridge : NotificationListenerService() {

    override fun onListenerConnected() {
        instance = this
        // The mirror must reflect reality after a rebind (service
        // restart, permission regrant): laptops replace-in-place on the
        // same key, so reposting everything active is convergence, not
        // spam. Goes through the same privacy gates as fresh posts.
        repostActive()
    }

    override fun onListenerDisconnected() {
        instance = null
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        noteApp(sbn)
        // Pulse only when the mirror truly carries it — a muted or
        // ongoing post the family never sees is not an event — and only
        // for fresh posts: the reconnect repost rides the same pipeline
        // but is convergence, not news.
        if (forward(sbn)) dev.murmur.core.EventPulses.emit(dev.murmur.core.PulseKind.NOTIFY)
    }

    /** The post pipeline, shared by fresh posts and the reconnect
     *  convergence repost — every gate lives exactly once. */
    private fun forward(sbn: StatusBarNotification): Boolean {
        if (sbn.packageName == packageName) return false // our own foreground notif
        if (sbn.isOngoing) return false // players, downloads — noise, not events
        val n = sbn.notification
        if (n.flags and Notification.FLAG_GROUP_SUMMARY != 0) return false // repeats its children
        // MediaStyle posts (track changed, album art refresh) are noise
        // here — the laptop already mirrors playback through its own
        // media widget with real controls. The token extra is the
        // reliable marker; ongoing-ness isn't (paused players drop it).
        if (n.extras.getParcelable<android.media.session.MediaSession.Token>(
                Notification.EXTRA_MEDIA_SESSION) != null) return false
        val svc = MurmurService.instance ?: return false
        if (!Secrets.mirrorFlow.value) return false
        if (sbn.packageName in Secrets.mutedFlow.value) return false
        if (n.visibility == Notification.VISIBILITY_SECRET) return false

        val title = n.extras.getCharSequence(Notification.EXTRA_TITLE)?.toString() ?: ""
        val body = if (Secrets.hideTextFlow.value) "" else
            n.extras.getCharSequence(Notification.EXTRA_TEXT)?.toString() ?: ""
        // Progress bars and "typing…" re-post the same key every second —
        // forward only real content changes.
        val sig = (title + "\u0000" + body).hashCode()
        if (forwarded.put(sbn.key, sig) == sig) return false
        svc.send(
            Msg.Notification(
                key = sbn.key,
                pkg = sbn.packageName,
                app = apps.value[sbn.packageName]?.label,
                title = title,
                body = body,
                icon_b64 = iconB64(avatarIcon(n)),
                actions = extractActions(n),
                ts = sbn.postTime / 1000,
            ),
        )
        return true
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        // Never forwarded (muted, secret, ongoing…) → laptops never saw it;
        // telling them would only leak that it existed.
        if (forwarded.remove(sbn.key) == null) return
        if (!remoteCancels.remove(sbn.key)) {
            MurmurService.instance?.send(Msg.NotificationDismiss(sbn.key))
        }
    }

    /** Remember the app for the "muted apps" picker — label from the
     *  ApplicationInfo the notification itself carries. */
    private fun noteApp(sbn: StatusBarNotification) {
        val pkg = sbn.packageName
        if (pkg == packageName || apps.value.containsKey(pkg)) {
            apps.value[pkg]?.let { apps.value = apps.value - pkg + (pkg to it.copy(seen = System.currentTimeMillis())) }
            return
        }
        val label = runCatching {
            packageManager.getApplicationLabel(packageManager.getApplicationInfo(pkg, 0)).toString()
        }.getOrNull() ?: pkg.substringAfterLast('.')
        apps.value = apps.value + (pkg to App(pkg, label, System.currentTimeMillis()))
    }

    /** The writer's avatar: MessagingStyle notifications (Telegram &
     *  co) carry it as the person icon of the newest message — a photo
     *  when the contact has one, a letter tile otherwise. Conversation
     *  and large icons are the fallbacks. */
    private fun avatarIcon(n: Notification): Icon? {
        val person = runCatching {
            // extractMessagingStyleFromNotification is @hide on modern
            // stubs — the public path is the messages extra itself.
            n.extras.getParcelableArray(Notification.EXTRA_MESSAGES)
                ?.let(MessagingStyle.Message::getMessagesFromBundleArray)
                ?.lastOrNull()?.senderPerson?.icon
        }.getOrNull()
        if (person != null) return person
        // "android.conversationIcon" — EXTRA_CONVERSATION_ICON is
        // @SystemApi on some SDK stubs; the literal is stable.
        return runCatching {
            n.extras.getParcelable<Icon>("android.conversationIcon")
        }.getOrNull() ?: n.getLargeIcon()
    }

    /** What the notification offers, in the laptop-facing shape. An
     *  action is a reply iff it carries a RemoteInput — the exact
     *  marker Wear OS / Android Auto use; the semantic label is the
     *  backup witness. Plain buttons are forwarded too: "mark as read"
     *  is worth clicking from a laptop. */
    private fun extractActions(n: Notification): List<dev.murmur.core.NotifAction> {
        val arr = n.actions ?: return emptyList()
        return arr.mapIndexedNotNull { i, a ->
            if (a?.actionIntent == null) return@mapIndexedNotNull null
            // Noise filter: mark-as-read is redundant on mirrors — opening
            // the notification on the phone reads the chat anyway, and the
            // button just eats the lane under the murmur tag.
            val t = a.title?.toString()?.lowercase() ?: ""
            if (!reply_capable(a) && t in NOISE_ACTIONS) return@mapIndexedNotNull null
            val reply = !a.remoteInputs.isNullOrEmpty() ||
                a.semanticAction == Notification.Action.SEMANTIC_ACTION_REPLY
            dev.murmur.core.NotifAction(
                index = i,
                title = a.title?.toString() ?: "",
                kind = if (reply) "reply" else "action",
            )
        }
    }

    /** The posted notification behind a key, or null if it's gone. */
    private fun findSbn(key: String): StatusBarNotification? =
        runCatching { activeNotifications.firstOrNull { it.key == key } }.getOrNull()

    /** largeIcon → downscaled PNG → b64 (bounded to ICON_PX). */
    private fun iconB64(icon: Icon?): String? {
        val d = runCatching { icon?.loadDrawable(this) }.getOrNull() ?: return null
        val scale = minOf(1f, ICON_PX.toFloat() / maxOf(d.intrinsicWidth, d.intrinsicHeight, 1))
        val w = (d.intrinsicWidth * scale).toInt().coerceAtLeast(1)
        val h = (d.intrinsicHeight * scale).toInt().coerceAtLeast(1)
        val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        Canvas(bmp).apply {
            d.setBounds(0, 0, w, h)
            d.draw(this)
        }
        val out = ByteArrayOutputStream()
        bmp.compress(Bitmap.CompressFormat.PNG, 100, out)
        bmp.recycle()
        return Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
    }

    data class App(val pkg: String, val label: String, val seen: Long)

    companion object {
        private const val ICON_PX = 96

        var instance: NotificationBridge? = null
            private set

        private val NOISE_ACTIONS = setOf(
            "mark as read", "mark read",
            "пометить прочитанным", "отметить прочитанным",
        )

        private fun reply_capable(a: Notification.Action): Boolean =
            !a.remoteInputs.isNullOrEmpty() ||
                a.semanticAction == Notification.Action.SEMANTIC_ACTION_REPLY

        /** Apps seen posting notifications this process — the picker's source. */
        val apps = MutableStateFlow<Map<String, App>>(emptyMap())

        /** Keys we forwarded → content signature (dedupe + dismiss gate). */
        private val forwarded = ConcurrentHashMap<String, Int>()

        /** Keys cancelled because a laptop asked — don't echo the removal. */
        private val remoteCancels = ConcurrentHashMap.newKeySet<String>()

/** Android 14+ blocks background activity launches even for listeners
     *  unless the send carries this explicit opt-in. Older APIs ignore the
     *  bundle entirely. */
    private fun balOptions(): android.os.Bundle? =
        if (android.os.Build.VERSION.SDK_INT >= 34) {
            android.app.ActivityOptions.makeBasic()
                .setPendingIntentBackgroundActivityStartMode(
                    android.app.ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED
                ).toBundle()
        } else null

    /** Fire contentIntent — the exact same PendingIntent a tap on the
     *  phone would fire. Listeners are exempt from background-launch
     *  restrictions; if the screen is off the app simply surfaces at
     *  unlock, which is what "open it on the phone" means anyway. */
        fun performOpen(key: String) {
        val sbn = instance?.findSbn(key) ?: return
        runCatching { sbn.notification.contentIntent?.send(instance, 0, null, null, null, null, balOptions()) }
            .onFailure { Log.w("NotificationBridge", "open failed: $it") }
    }

    /** Fire a plain action button by its original positional index. */
        fun performAction(key: String, index: Int) {
        val sbn = instance?.findSbn(key) ?: return
        val a = sbn.notification.actions?.getOrNull(index) ?: return
        runCatching { a.actionIntent?.send(instance, 0, null, null, null, null, balOptions()) }
            .onFailure { Log.w("NotificationBridge", "action failed: $it") }
    }

    /** Fill the reply action's RemoteInput with [text] and fire it —
     *  byte-identical to what the shade's inline field does, so the
     *  app can't tell the difference (and neither can the chat peer). */
        fun performReply(key: String, text: String) {
        val sbn = instance?.findSbn(key) ?: return
        val actions = sbn.notification.actions ?: return
        val reply = actions.firstOrNull {
            !it.remoteInputs.isNullOrEmpty() ||
                it.semanticAction == Notification.Action.SEMANTIC_ACTION_REPLY
        } ?: return
        val inputs = reply.remoteInputs ?: return
        val intent = android.content.Intent()
        val results = android.os.Bundle()
        for (input in inputs) {
            results.putCharSequence(input.resultKey, text)
        }
        android.app.RemoteInput.addResultsToIntent(inputs, intent, results)
        runCatching { reply.actionIntent.send(instance, 0, intent, null, null, null, balOptions()) }
            .onFailure { Log.w("NotificationBridge", "reply failed: $it") }
    }


        /** The mirror must reflect reality after any restart — laptops
         *  replace-in-place on the same key, so reposting everything
         *  active is convergence, not spam. Safe to call from either side
         *  of the startup race: gates no-op until both the listener and
         *  MurmurService exist. */
        fun repostActive() {
            val self = instance ?: return
            runCatching { self.activeNotifications }.getOrNull()?.forEach { sbn ->
                self.noteApp(sbn)
                self.forward(sbn)
            }
        }

        fun requestCancel(key: String) {
            val listener = instance ?: return
            remoteCancels.add(key)
            listener.cancelNotification(key)
        }
    }
}
