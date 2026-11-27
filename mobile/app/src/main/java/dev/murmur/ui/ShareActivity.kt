package dev.murmur.ui

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast

/**
 * Share-sheet target: "send to laptop" for text and links.
 * Transparent, does its thing, finishes.
 */
class ShareActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Toast.makeText(
            this,
            SharePush.handle(this, intent.getStringExtra(Intent.EXTRA_TEXT)),
            Toast.LENGTH_SHORT,
        ).show()
        finish()
    }
}
