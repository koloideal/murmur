package dev.murmur.ui.widgets

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Hub
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.foundation.shape.RoundedCornerShape
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
    val removed by Secrets.removedFlow.collectAsState()

    var editingName by remember { mutableStateOf(false) }
    var confirmLeave by remember { mutableStateOf(false) }
    var deviceName by remember { mutableStateOf(secrets.deviceName) }

    // In a serverless family there is no relay — "online" means a
    // peer is actually linked over LAN.
    val online = if (status.familyKind == "serverless") {
        status.lanPeers.isNotEmpty()
    } else {
        status.relay == Relay.State.ONLINE
    }

    val (color, label) = when {
        removed -> MaterialTheme.colorScheme.error to "removed"
        !status.running -> MaterialTheme.colorScheme.onSurfaceVariant to "paused"
        online -> MaterialTheme.colorScheme.secondary to "online"
        status.relay == Relay.State.CONNECTING ->
            MaterialTheme.colorScheme.primary to "connecting"
        else -> MaterialTheme.colorScheme.error to "offline"
    }

    // The wave breathes while the link is alive.
    val phase by if (status.running && online) {
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
                // Live link → the wave bleeds to the card's edges (the
                // column's 20dp inner padding negated); every other
                // state keeps the shared content padding.
                val waveBleed by animateDpAsState(
                    if (online) (-20).dp else 0.dp,
                    label = "waveBleed",
                )
                Canvas(
                    Modifier
                        .fillMaxWidth()
                        .padding(horizontal = waveBleed)
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
                if (removed) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "the relay no longer knows this phone — it was removed " +
                            "from the family. Its keys are useless now.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                    Spacer(Modifier.height(14.dp))
                    Button(onClick = { secrets.clear() }) { Text("pair again") }
                    return@Column
                }
                if (status.running && status.relay == Relay.State.OFFLINE &&
                    status.familyKind != "serverless"
                ) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "can't reach the relay — is it running?",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.height(16.dp))
                StatusRow(
                    "family",
                    status.familyName ?: "—",
                    dim = status.familyName == null,
                )

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
                StatusRow(
                    "server",
                    if (status.familyKind == "serverless") "none — lan only"
                    else secrets.serverUrl.ifEmpty { "—" },
                )
                StatusRow(
                    "lan addr",
                    status.lanAddr ?: "—",
                    dim = status.lanAddr == null,
                )
                if (status.running && status.familyKind == "serverless" &&
                    status.lanPeers.isEmpty() && status.lanAddr != null
                ) {
                    Text(
                        "no peers found — on filtered Wi-Fi discovery can't " +
                            "pass. Dial this phone from the laptop:",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(6.dp))
                    CommandLine("murmurctl dial ${status.lanAddr}")
                    Spacer(Modifier.height(4.dp))
                }
                if (status.familyKind != "serverless" && secrets.serverUrl.startsWith("ws://")) {
                    Text(
                        "plain ws — messages stay end-to-end sealed, but the " +
                            "relay login travels unencrypted. Prefer wss://.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.tertiary,
                        modifier = Modifier.fillMaxWidth().padding(bottom = 4.dp),
                    )
                }
                StatusRow(
                    "ping",
                    if (status.familyKind == "serverless") {
                        status.lanPeers.values.filter { it > 0 }.maxOrNull()
                            ?.let { "$it ms" } ?: "—"
                    } else {
                        status.serverRttMs?.let { "$it ms" } ?: "—"
                    },
                    dim = if (status.familyKind == "serverless")
                        status.lanPeers.values.all { it <= 0 }
                    else status.serverRttMs == null,
                )
                Spacer(Modifier.height(8.dp))
                Row {
                    OutlinedButton(
                        onClick = {
                            if (status.running) MurmurService.stop(ctx)
                            else MurmurService.start(ctx)
                        },
                        shape = RoundedCornerShape(50),
                        border = BorderStroke(1.dp, MaterialTheme.colorScheme.primary),
                    ) {
                        Text(
                            if (status.running) "pause" else "resume",
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }
                    Spacer(Modifier.weight(1f))
                    OutlinedButton(
                        onClick = { confirmLeave = true },
                        shape = RoundedCornerShape(50),
                        border = BorderStroke(1.dp, MaterialTheme.colorScheme.error),
                    ) {
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
        val online = status.running && status.relay == Relay.State.ONLINE
        AlertDialog(
            onDismissRequest = { confirmLeave = false },
            title = { Text("leave the family?") },
            text = {
                Column {
                    Text(
                        "This wipes the phone's keys and revokes it on the relay; " +
                            "the laptops then rotate the family key. You'll need a " +
                            "fresh invite to pair again.",
                    )
                    if (!online) {
                        Spacer(Modifier.height(12.dp))
                        Text(
                            "You're offline, so the relay won't hear about it — this " +
                                "phone stays in the family until you remove it on the " +
                                "relay host:",
                            color = MaterialTheme.colorScheme.error,
                        )
                        Spacer(Modifier.height(8.dp))
                        CommandLine("sudo murmurs revoke --id ${secrets.deviceId}")
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    confirmLeave = false
                    MurmurService.instance?.revokeSelf()
                    MurmurService.stop(ctx)
                    secrets.clear()
                }) {
                    Text(
                        if (online) "leave" else "leave anyway",
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmLeave = false }) { Text("stay") }
            },
        )
    }
}
