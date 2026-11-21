package dev.murmur.bridge

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.drawable.Icon
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Base64
import dev.murmur.MurmurService
import dev.murmur.core.Msg
import java.io.ByteArrayOutputStream
import java.util.concurrent.ConcurrentHashMap

/**
 * System-bound notification listener — the source of truth for what the
 * phone shows. Posted → broadcast; removed → dismiss everywhere, unless we
 * cancelled it ourselves on a remote dismiss (anti-loop via [remoteCancels]).
 */
class NotificationBridge : NotificationListenerService() {

    override fun onListenerConnected() {
        instance = this
    }

    override fun onListenerDisconnected() {
        instance = null
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        val svc = MurmurService.instance ?: return
        if (sbn.packageName == packageName) return // our own foreground notif
        if (sbn.isOngoing) return // players, downloads — noise, not events
        if (!allowlisted(sbn.packageName)) return
        val n = sbn.notification
        svc.send(
            Msg.Notification(
                key = sbn.key,
                pkg = sbn.packageName,
                title = n.extras.getString("android.title") ?: "",
                body = n.extras.getString("android.text") ?: "",
                icon_b64 = iconB64(n.getLargeIcon()),
                ts = sbn.postTime / 1000,
            ),
        )
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        if (!remoteCancels.remove(sbn.key)) {
            MurmurService.instance?.send(Msg.NotificationDismiss(sbn.key))
        }
    }

    private fun allowlisted(pkg: String): Boolean {
        val allow = MurmurService.instance?.let {
            dev.murmur.core.Secrets(it).notificationAllowlist
        } ?: return true
        return allow.isEmpty() || pkg in allow
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

    companion object {
        private const val ICON_PX = 96

        var instance: NotificationBridge? = null
            private set

        /** Keys cancelled because a laptop asked — don't echo the removal. */
        private val remoteCancels = ConcurrentHashMap.newKeySet<String>()

        fun requestCancel(key: String) {
            val listener = instance ?: return
            remoteCancels.add(key)
            listener.cancelNotification(key)
        }
    }
}
