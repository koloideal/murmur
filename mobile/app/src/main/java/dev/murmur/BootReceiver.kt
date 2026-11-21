package dev.murmur

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Restart the agent after reboot. No-ops until the device is provisioned. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            MurmurService.start(context)
        }
    }
}
