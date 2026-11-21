package dev.murmur.bridge

import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import dev.murmur.MurmurService
import dev.murmur.core.Msg
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
        if (!allowlisted(sbn.packageName)) return
        val n = sbn.notification
        svc.send(
            Msg.Notification(
                key = sbn.key,
                pkg = sbn.packageName,
                title = n.extras.getString("android.title") ?: "",
                body = n.extras.getString("android.text") ?: "",
                icon_b64 = null, // TODO: largeIcon → PNG → b64
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

    companion object {
        var instance: NotificationBridge? = null
            private set

        /** Keys cancelled because a laptop asked — don't echo the removal. */
        private val remoteCancels = ConcurrentHashMap.newKeySet<String>()

        fun requestCancel(key: String) {
            remoteCancels.add(key)
            instance?.cancelNotification(key)
        }
    }
}
