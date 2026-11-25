package dev.murmur

import android.app.NotificationManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** "exit" on the paused notification — the service is already gone,
 *  so all that's left is to take the notification down. */
class ExitReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        context.getSystemService(NotificationManager::class.java)
            .cancel(MurmurService.PAUSED_ID)
    }
}
