package dev.murmur.ui

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import dev.murmur.MurmurService

/**
 * Custom action in the text-selection menu: selected text → active laptop.
 */
class ProcessTextActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
        if (text.isNullOrBlank()) {
            toast("nothing selected")
        } else if (MurmurService.instance == null) {
            MurmurService.start(this)
            toast("murmur is starting — try again in a second")
        } else {
            MurmurService.instance!!.pushClipboard(text)
            toast("sent to laptop")
        }
        setResult(RESULT_OK, Intent().putExtra(Intent.EXTRA_PROCESS_TEXT, text))
        finish()
    }

    private fun toast(s: String) = Toast.makeText(this, s, Toast.LENGTH_SHORT).show()
}
