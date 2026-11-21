package dev.murmur.ui

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import dev.murmur.MurmurService
import dev.murmur.core.Secrets

/**
 * Setup screen: the only UI this app has. Grants the three things the agent
 * needs — notification access, battery exemption, a running service.
 */
class MainActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val status = TextView(this).apply {
            text = statusText()
            setPadding(48, 48, 48, 24)
        }

        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(status)
            addView(action("notification access") {
                startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
            })
            addView(action("exclude from battery optimization") {
                startActivity(
                    Intent(
                        Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                        Uri.parse("package:$packageName"),
                    ),
                )
            })
            addView(action("start murmur") {
                MurmurService.start(this@MainActivity)
            })
        }
        setContentView(layout)
    }

    override fun onResume() {
        super.onResume()
        // TODO: bind status TextView to relay.state
    }

    private fun statusText(): String {
        val secrets = Secrets(this)
        return buildString {
            append("murmur\n\n")
            append("provisioned: ${secrets.provisioned}\n")
            append("device: ${secrets.deviceId ?: "—"}\n")
            append("server: ${secrets.serverUrl}\n")
            append("service: ${if (MurmurService.instance != null) "running" else "stopped"}\n")
        }
    }

    private fun action(label: String, onClick: () -> Unit) =
        Button(this).apply {
            text = label
            setOnClickListener { onClick() }
        }
}
