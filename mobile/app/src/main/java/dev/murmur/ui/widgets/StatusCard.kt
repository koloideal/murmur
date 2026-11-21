package dev.murmur.ui.widgets

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Hub
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.murmur.MurmurService
import dev.murmur.core.Relay
import dev.murmur.core.Secrets

/**
 * The dashboard hero: relay state as a big centered word over a living
 * whisper wave, then identity, ping, and the service's own controls
 * (pause/resume, rename, leave). Hidden until [Secrets.settledFlow].
 */
@Composable
fun StatusCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val status by MurmurService.status.collectAsState()
    val settled by Secrets.settledFlow.collectAsState()

    var editingName by remember { mutableStateOf(false) }
    var confirmLeave by remember { mutableStateOf(false) }
    var deviceName by remember { mutableStateOf(secrets.deviceName) }

    val (color, label) = when {
        !status.running -> MaterialTheme.colorScheme.onSurfaceVariant to "paused"
        status.relay == Relay.State.ONLINE ->
            MaterialTheme.colorScheme.secondary to "online"
        status.relay == Relay.State.CONNECTING ->
            MaterialTheme.colorScheme.primary to "connecting"
        else -> MaterialTheme.colorScheme.error to "offline"
    }

    // The wave breathes while the relay is online.
    val phase by if (status.running && status.relay == Relay.State.ONLINE) {
        rememberInfiniteTransition(label = "wave").animateFloat(
            initialValue = 0f,
            targetValue = 6.2832f,
            animationSpec = infiniteRepeatable(
                tween(2600, easing = LinearEasing), RepeatMode.Restart,
            ),
            label = "phase",
        )
    } else {
        remember { mutableStateOf(0f) }
    }

    AnimatedVisibility(
        visible = settled,
        enter = fadeIn(tween(600)) + expandVertically(tween(600)),
    ) {
        FeatureCard("murmur", Icons.Outlined.Hub) {
            Column(
                Modifier.fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Canvas(
                    Modifier
                        .fillMaxWidth()
                        .height(56.dp),
                ) {
                    drawPath(
                        wavePath(size.width, size.height / 2f,
                            amplitude = 13.dp.toPx(),
                            wavelength = size.width / 3.2f,
                            phase = phase),
                        color.copy(alpha = 0.85f),
                        style = Stroke(width = 4.dp.toPx()),
                    )
                }
                Text(
                    label,
                    style = MaterialTheme.typography.displaySmall,
                    color = color,
                )
                if (status.running && status.relay == Relay.State.OFFLINE) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "can't reach the relay — is it running?",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.height(16.dp))

                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        StatusRow("name", deviceName)
                    }
                    IconButton(onClick = { editingName = true }) {
                        Icon(
                            Icons.Outlined.Edit,
                            contentDescription = "rename this device",
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                StatusRow("device", secrets.deviceId ?: "—", dim = secrets.deviceId == null)
                StatusRow("server", secrets.serverUrl.ifEmpty { "—" })
                StatusRow(
                    "ping",
                    status.serverRttMs?.let { "$it ms" } ?: "—",
                    dim = status.serverRttMs == null,
                )
                Spacer(Modifier.height(8.dp))
                Row {
                    TextButton(onClick = {
                        if (status.running) MurmurService.stop(ctx) else MurmurService.start(ctx)
                    }) {
                        Text(if (status.running) "pause" else "resume")
                    }
                    Spacer(Modifier.weight(1f))
                    TextButton(onClick = { confirmLeave = true }) {
                        Text(
                            "leave the family",
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                }
            }
        }
    }

    if (editingName) {
        var name by remember { mutableStateOf(deviceName) }
        AlertDialog(
            onDismissRequest = { editingName = false },
            title = { Text("device name") },
            text = {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    singleLine = true,
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    deviceName = name.trim().ifEmpty { deviceName }
                    secrets.deviceName = deviceName
                    MurmurService.instance?.pushName()
                    editingName = false
                }) { Text("save") }
            },
            dismissButton = {
                TextButton(onClick = { editingName = false }) { Text("cancel") }
            },
        )
    }

    if (confirmLeave) {
        AlertDialog(
            onDismissRequest = { confirmLeave = false },
            title = { Text("leave the family?") },
            text = {
                Text(
                    "This wipes the phone's keys and revokes it on the relay. " +
                        "You'll need a fresh invite to pair again.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    confirmLeave = false
                    MurmurService.instance?.revokeSelf()
                    MurmurService.stop(ctx)
                    secrets.clear()
                }) {
                    Text("leave", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmLeave = false }) { Text("stay") }
            },
        )
    }
}
