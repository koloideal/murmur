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
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.ui.layout.layout
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

    var confirmLeave by remember { mutableStateOf(false) }
    var deviceName by remember { mutableStateOf(secrets.deviceName) }
    // The family kind is a pairing fact — a paused service publishes a
    // bare Status, and the dashboard must not flip to relay mode then.
    val serverless = secrets.familyKind == "serverless"

    // In a serverless family there is no relay — "online" means a
    // peer is actually linked over LAN.
    val online = if (serverless) {
        status.lanPeers.isNotEmpty()
    } else {
        status.relay == Relay.State.ONLINE
    }

    // Starting a link takes seconds (discovery → dial → first frame) —
    // "offline" right after resume would be a lie, so there's a grace.
    val connectingGrace = status.running && !online &&
        android.os.SystemClock.elapsedRealtime() - status.startedAtMs < 12_000

    val (color, label) = when {
        removed -> MaterialTheme.colorScheme.error to "removed"
        !status.running -> MaterialTheme.colorScheme.onSurfaceVariant to "paused"
        online -> MaterialTheme.colorScheme.secondary to "online"
        connectingGrace || status.relay == Relay.State.CONNECTING ->
            MaterialTheme.colorScheme.primary to "connecting"
        else -> MaterialTheme.colorScheme.error to "offline"
    }

    // The wave breathes while the link is alive; while connecting it
    // still moves — searching is motion too — just three times slower.
    val connecting = status.running && !online &&
        (connectingGrace || status.relay == Relay.State.CONNECTING)
    val phase by if (online || connecting) {
        rememberInfiniteTransition(label = "wave").animateFloat(
            initialValue = 0f,
            targetValue = 6.2832f,
            animationSpec = infiniteRepeatable(
                tween(if (online) 2600 else 8000, easing = LinearEasing),
                RepeatMode.Restart,
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
                    if (online) 20.dp else 0.dp,
                    label = "waveBleed",
                )
                Canvas(
                    Modifier
                        .fillMaxWidth()
                        // Padding can't go negative — widen the measured
                        // box past the parent and re-center it instead.
                        .layout { m, c ->
                            val extra = waveBleed.roundToPx()
                            val w = c.maxWidth + 2 * extra
                            val p = m.measure(c.copy(minWidth = w, maxWidth = w))
                            layout(c.maxWidth, p.height) { p.place(-extra, 0) }
                        }
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
                    !serverless
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
                    status.familyName ?: secrets.familyName ?: "—",
                    dim = (status.familyName ?: secrets.familyName) == null,
                )

                NameRow(deviceName) { n ->
                    secrets.deviceName = n
                    deviceName = secrets.deviceName
                    MurmurService.instance?.pushName()
                }
                if (!serverless) {
                    StatusRow("server", secrets.serverUrl.ifEmpty { "—" })
                }
                if (status.running) {
                    StatusRow(
                        "lan addr",
                        status.lanAddr ?: "—",
                        dim = status.lanAddr == null,
                    )
                }
                if (status.running && serverless &&
                    status.lanPeers.isEmpty() && status.lanAddr != null
                ) {
                    Spacer(Modifier.height(14.dp))
                    Text(
                        "no peers — filtered Wi-Fi blocks discovery. " +
                            "Dial this phone from the laptop:",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(6.dp))
                    CommandLine(
                        "murmurctl dial ${status.lanAddr}",
                        Modifier.fillMaxWidth(),
                        singleLine = true,
                    )
                    Spacer(Modifier.height(14.dp))
                }
                if (!serverless && secrets.serverUrl.startsWith("ws://")) {
                    Text(
                        "plain ws — messages stay end-to-end sealed, but the " +
                            "relay login travels unencrypted. Prefer wss://.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.tertiary,
                        modifier = Modifier.fillMaxWidth().padding(bottom = 4.dp),
                    )
                }
                if (!serverless) {
                    StatusRow(
                        "ping",
                        status.serverRttMs?.let { "$it ms" } ?: "—",
                        dim = status.serverRttMs == null,
                    )
                }
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

/** The "name" row, edited in place: tap the value → it becomes a field
 *  of the same text style and height; Done or focus loss saves, an
 *  empty entry keeps the old name. */
@Composable
private fun NameRow(name: String, onSave: (String) -> Unit) {
    var editing by remember { mutableStateOf(false) }
    var draft by remember(name) { mutableStateOf(name) }
    val focus = remember { FocusRequester() }
    // onFocusChanged fires once unfocused on attach — only a real
    // focus → blur transition may commit.
    var focused by remember { mutableStateOf(false) }
    val style = MaterialTheme.typography.bodyMedium
    fun commit() {
        if (!editing) return
        editing = false
        focused = false
        draft.trim().takeIf { it.isNotEmpty() && it != name }?.let(onSave) ?: run { draft = name }
    }
    Row(
        Modifier.fillMaxWidth().padding(vertical = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("name", style = style, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(16.dp))
        if (editing) {
            BasicTextField(
                value = draft,
                onValueChange = { draft = it },
                singleLine = true,
                textStyle = style.copy(
                    color = MaterialTheme.colorScheme.primary,
                    textAlign = TextAlign.End,
                ),
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { commit() }),
                modifier = Modifier
                    .weight(1f)
                    .focusRequester(focus)
                    .onFocusChanged {
                        if (it.isFocused) focused = true
                        else if (focused) commit()
                    },
            )
            LaunchedEffect(Unit) { focus.requestFocus() }
        } else {
            Row(
                Modifier.weight(1f).clickable { editing = true },
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(name, style = style, color = MaterialTheme.colorScheme.onSurface,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.width(6.dp))
                Icon(
                    Icons.Outlined.Edit,
                    contentDescription = "rename this device",
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(14.dp),
                )
            }
        }
    }
}
