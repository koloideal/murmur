package dev.murmur.ui.widgets

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Send
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.murmur.MurmurService
import dev.murmur.core.Secrets

/**
 * Clipboard in both directions: a master switch, then per-direction
 * rows while it's on. The bottom half is the answer to "how do I copy
 * phone → laptop at all": paste here and send — Android gives
 * background apps no way to read the clipboard, so the system paths
 * (selection menu / share sheet) and this field are the only doors.
 */
@Composable
fun ClipboardCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val settled by Secrets.settledFlow.collectAsState()
    val removed by Secrets.removedFlow.collectAsState()
    val enabled by Secrets.clipFlow.collectAsState()
    val recv by Secrets.clipRecvFlow.collectAsState()
    val sendOn by Secrets.clipSendFlow.collectAsState()

    AnimatedVisibility(
        visible = settled && !removed,
        enter = fadeIn(tween(600)) + expandVertically(tween(600)),
    ) {
        FeatureCard("clipboard", Icons.Outlined.ContentPaste) {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ToggleRow(
                    "sync clipboard",
                    "share the clipboard with your laptops",
                    checked = enabled,
                    onChange = { secrets.clipEnabled = it },
                )
                AnimatedVisibility(
                    visible = enabled,
                    enter = fadeIn(tween(250)) + expandVertically(tween(250)),
                    exit = fadeOut(tween(200)) + shrinkVertically(tween(200)),
                ) {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        ToggleRow(
                            "laptop → phone",
                            "clips from the laptop you're at land here",
                            checked = recv,
                            onChange = { secrets.clipRecv = it },
                        )
                        ToggleRow(
                            "phone → laptop",
                            "text you send lands on the active laptop",
                            checked = sendOn,
                            onChange = { secrets.clipSend = it },
                        )
                        if (sendOn) {
                            SendRow()
                            Text(
                                "select → ⋮ / share → Send to laptop",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(horizontal = 4.dp),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SendRow() {
    val ctx = LocalContext.current
    var text by remember { mutableStateOf("") }
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = text,
            onValueChange = { text = it },
            modifier = Modifier.weight(1f),
            placeholder = {
                Text(
                    "paste → send to laptop",
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = 1,
                )
            },
            singleLine = true,
            textStyle = MaterialTheme.typography.bodyMedium,
        )
        IconButton(
            onClick = {
                val t = text.trim()
                if (t.isNotEmpty()) {
                    val svc = MurmurService.instance
                    val st = svc?.let { MurmurService.status.value }
                    val linked = st != null &&
                        (st.relay == dev.murmur.core.Relay.State.ONLINE || st.lanPeers.isNotEmpty())
                    val msg = when {
                        svc == null -> "murmur isn't running"
                        !linked -> "offline — not sent"
                        else -> {
                            svc.pushClipboard(t)
                            text = ""
                            "sent to laptop"
                        }
                    }
                    android.widget.Toast.makeText(
                        ctx, msg, android.widget.Toast.LENGTH_SHORT,
                    ).show()
                }
            },
        ) {
            Icon(
                Icons.AutoMirrored.Outlined.Send,
                contentDescription = "send",
                tint = MaterialTheme.colorScheme.primary,
            )
        }
    }
}
