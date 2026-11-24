package dev.murmur.bridge

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.util.Base64
import android.util.Log
import dev.murmur.core.Msg

/**
 * Applies incoming Clipboard messages to the phone clipboard.
 * Writes are allowed from background on Android 10+; the system may show a
 * small preview chip — cosmetic, verify on-device.
 */
object ClipboardBridge {

    fun apply(ctx: Context, msg: Msg.Clipboard) {
        val data = Base64.decode(msg.data_b64, Base64.NO_WRAP)
        // Senders annotate text mimes ("text/plain;charset=utf-8") —
        // match on the base type.
        val clip = when (msg.mime.substringBefore(';').trim()) {
            "text/plain", "text/uri-list" ->
                ClipData.newPlainText("murmur", data.decodeToString()).apply {
                    // Passive sync lands while the phone may be face-up on
                    // a desk — keep the system's copy preview from
                    // flashing laptop clipboard contents on screen.
                    description.extras = android.os.PersistableBundle().apply {
                        putBoolean(android.content.ClipDescription.EXTRA_IS_SENSITIVE, true)
                    }
                }
            "image/png" -> {
                // TODO: write to cache, hand out a content Uri via FileProvider
                return
            }
            else -> {
                Log.d("ClipboardBridge", "dropped mime ${msg.mime}")
                return
            }
        }
        ctx.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
        Log.d("ClipboardBridge", "applied ${data.size}B ${msg.mime}")
    }
}
