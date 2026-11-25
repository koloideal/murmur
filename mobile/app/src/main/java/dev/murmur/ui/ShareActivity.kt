package dev.murmur.ui

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import dev.murmur.MurmurService

/**
 * Share-sheet target: "send to laptop" for text and links.
 * Transparent, does its thing, finishes.
 */
class ShareActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)
        when {
            text.isNullOrBlank() -> toast("nothing to send")
            MurmurService.instance == null -> {
                // TODO: queue the push and deliver once the service is up
                MurmurService.start(this)
                toast("murmur is starting — share again in a second")
            }
            else -> {
                val st = MurmurService.status.value
                // Serverless has no relay — a live LAN peer delivers.
                if (st.relay != dev.murmur.core.Relay.State.ONLINE && st.lanPeers.isEmpty()) {
                    toast("offline — not sent")
                } else {
                    MurmurService.instance!!.pushClipboard(text)
                    toast("sent to laptop")
                }
            }
        }
        finish()
    }

    private fun toast(s: String) = Toast.makeText(this, s, Toast.LENGTH_SHORT).show()
}
