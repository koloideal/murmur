package dev.murmur.bridge

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.util.Base64
import dev.murmur.core.Msg

/**
 * Applies incoming Clipboard messages to the phone clipboard.
 * Writes are allowed from background on Android 10+; the system may show a
 * small preview chip — cosmetic, verify on-device.
 */
object ClipboardBridge {

    fun apply(ctx: Context, msg: Msg.Clipboard) {
        val data = Base64.decode(msg.data_b64, Base64.NO_WRAP)
        val clip = when (msg.mime) {
            "text/plain", "text/uri-list" ->
                ClipData.newPlainText("murmur", data.decodeToString())
            "image/png" -> {
                // TODO: write to cache, hand out a content Uri via FileProvider
                return
            }
            else -> return
        }
        ctx.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
    }
}
