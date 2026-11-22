package dev.murmur.bridge

import android.app.Notification
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.drawable.Icon
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Base64
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
 * Privacy gates, in order: ongoing/group-summary noise, muted apps,
 * `VISIBILITY_SECRET` (the app itself says "not even on a lock screen"),
 * then the optional "hide message text" mode. Android 15+ additionally
 * redacts OTPs for listeners like us before we ever see them.
 */
class NotificationBridge : NotificationListenerService() {

    override fun onListenerConnected() {
        instance = this
        runCatching { activeNotifications }.getOrNull()?.forEach { noteApp(it) }
    }

    override fun onListenerDisconnected() {
        instance = null
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        if (sbn.packageName == packageName) return // our own foreground notif
        if (sbn.isOngoing) return // players, downloads — noise, not events
        val n = sbn.notification
        if (n.flags and Notification.FLAG_GROUP_SUMMARY != 0) return // repeats its children
        noteApp(sbn)
        val svc = MurmurService.instance ?: return
        if (sbn.packageName in Secrets.mutedFlow.value) return
        if (n.visibility == Notification.VISIBILITY_SECRET) return

        val title = n.extras.getCharSequence(Notification.EXTRA_TITLE)?.toString() ?: ""
        val body = if (Secrets.hideTextFlow.value) "" else
            n.extras.getCharSequence(Notification.EXTRA_TEXT)?.toString() ?: ""
        // Progress bars and "typing…" re-post the same key every second —
        // forward only real content changes.
        val sig = (title + "\u0000" + body).hashCode()
        if (forwarded.put(sbn.key, sig) == sig) return
        svc.send(
            Msg.Notification(
                key = sbn.key,
                pkg = sbn.packageName,
                app = apps.value[sbn.packageName]?.label,
                title = title,
                body = body,
                icon_b64 = iconB64(n.getLargeIcon()),
                ts = sbn.postTime / 1000,
            ),
        )
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

        /** Apps seen posting notifications this process — the picker's source. */
        val apps = MutableStateFlow<Map<String, App>>(emptyMap())

        /** Keys we forwarded → content signature (dedupe + dismiss gate). */
        private val forwarded = ConcurrentHashMap<String, Int>()

        /** Keys cancelled because a laptop asked — don't echo the removal. */
        private val remoteCancels = ConcurrentHashMap.newKeySet<String>()

        fun requestCancel(key: String) {
            val listener = instance ?: return
            remoteCancels.add(key)
            listener.cancelNotification(key)
        }
    }
}
