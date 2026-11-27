package dev.murmur.bridge

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.util.Base64
import android.util.Log
import androidx.core.content.FileProvider
import dev.murmur.core.Msg
import java.io.File

/**
 * Applies incoming Clipboard messages to the phone clipboard.
 * Writes are allowed from background on Android 10+; the system may show a
 * small preview chip — cosmetic, verify on-device.
 *
 * Images land as a cache-backed content Uri via FileProvider — the
 * pasting app gets a temporary read grant through the clipboard.
 */
object ClipboardBridge {

    /** Received images outlive the paste by a day, then get swept — an
 *  hour was short enough that a clipboard entry could die before a
 *  perfectly reasonable paste. */
    private const val CLIP_TTL_MS = 86_400_000L

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
            "image/png" -> ClipData.newUri(ctx.contentResolver, "murmur", cacheImage(ctx, data))
            else -> {
                Log.d("ClipboardBridge", "dropped mime ${msg.mime}")
                return
            }
        }
        ctx.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
        Log.d("ClipboardBridge", "applied ${data.size}B ${msg.mime}")
    }

    /** Empty the clipboard — the laptop user erased it there, so it
     *  stops existing here too. clearPrimaryClip is API 28+, we're 26+. */
    fun applyClear(ctx: Context) {
        runCatching {
            ctx.getSystemService(ClipboardManager::class.java).clearPrimaryClip()
        }.onSuccess { Log.d("ClipboardBridge", "cleared") }
    }

    /** Write the PNG under cacheDir/clips and hand out a FileProvider Uri. */
    private fun cacheImage(ctx: Context, png: ByteArray): android.net.Uri {
        val dir = File(ctx.cacheDir, "clips").apply { mkdirs() }
        val cutoff = System.currentTimeMillis() - CLIP_TTL_MS
        dir.listFiles()?.forEach { if (it.lastModified() < cutoff) it.delete() }
        val f = File(dir, "clip-${System.currentTimeMillis()}.png")
        f.writeBytes(png)
        return FileProvider.getUriForFile(ctx, "${ctx.packageName}.fileprovider", f)
    }
}
