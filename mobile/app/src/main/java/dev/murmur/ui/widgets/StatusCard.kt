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
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.layout
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import kotlin.math.PI
import kotlin.math.sin
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.murmur.MurmurService
import dev.murmur.core.EventPulses
import dev.murmur.core.PulseKind
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
    // The clock ticks once a second — without it the grace check is
    // frozen at whatever realtime the last recomposition saw, and
    // "connecting" would stick long past its 12 seconds.
    var nowMs by remember { mutableStateOf(android.os.SystemClock.elapsedRealtime()) }
    LaunchedEffect(status.startedAtMs, online) {
        while (!online) {
            nowMs = android.os.SystemClock.elapsedRealtime()
            kotlinx.coroutines.delay(1_000)
        }
    }
    val connectingGrace = status.running && !online &&
        nowMs - status.startedAtMs < 12_000

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
                // Event pulses — one real occurrence (clip applied,
                // notification mirrored, track boundary) = exactly one
                // amplitude swell: the wave inhales once and settles.
                // Emission happens only at feature truth-points, so
                // heartbeats/reposts can't fake liveliness here.
                val pulses = remember { mutableStateListOf<GhostPulse>() }
                LaunchedEffect(Unit) {
                    EventPulses.flow.collect { kind ->
                        val now = System.currentTimeMillis()
                        // Each pulse rolls its own shape once: jump height
                        // and packet width vary within the range that
                        // stays inside the card, so a burst of events
                        // reads as distinct leaps, not one blob.
                        pulses.add(GhostPulse(
                            t0 = now,
                            kind = kind,
                            crest = (0..2).random(),
                            bulge = (60..130).random().toFloat() / 100f,
                        ))
                        pulses.removeAll { now - it.t0 > PULSE_MS }
                    }
                }
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
                    // The main wave never reacts — liveliness is the
                    // ghosts'. One event = one wave-packet torn off the
                    // wave: same sine, windowed to a central piece (Hann
                    // taper, so it reads as a chunk of the wave rather
                    // than a clipped copy), dimmer, jumping up off the
                    // baseline and settling back onto it — sin(pi·p) is
                    // zero at both ends by construction. Kind shows only
                    // as a faint tint in the ghost.
                    val nowMs = System.currentTimeMillis()
                    val amp = 13.dp.toPx()
                    val wl = size.width / 3.2f
                    // One event = one crest of the wave itself, bulging
                    // upward. The ghost copies exactly one period of the
                    // same sine between two zero crossings — so its ends
                    // never leave the baseline at any phase — while its
                    // middle swells with the envelope. The crest it
                    // haunted stays put; what you see is the wave's own
                    // echo stretched taller, dimmer, tinted by kind.
                    val tau = (2f * PI).toFloat()
                    for (pulse in pulses) {
                        val p = (nowMs - pulse.t0) / PULSE_MS.toFloat()
                        if (p !in 0f..1f) continue
                        val env = sin(PI.toFloat() * p)
                        val tint = when (pulse.kind) {
                            PulseKind.CLIP -> PULSE_CLIP
                            PulseKind.NOTIFY -> PULSE_NOTIFY
                            PulseKind.MEDIA -> PULSE_MEDIA
                        }
                        // Zero crossings: tau·x/wl + phase = m·π. Collect
                        // whole periods fully inside the width, haunt the
                        // pulse's rolled one.
                        val periods = mutableListOf<Pair<Float, Float>>()
                        // x = (mπ − phase)·wl/τ is non-negative from
                        // m ≥ phase/π — start one earlier and scan up.
                        var m = kotlin.math.ceil(phase / Math.PI).toInt() - 1
                        while (true) {
                            val x0 = (m * Math.PI - phase).toFloat() * wl / tau
                            val x1 = ((m + 2) * Math.PI - phase).toFloat() * wl / tau
                            if (x1 < 0f) { m++; continue }
                            if (x0 > size.width) break
                            if (x0 >= 0f && x1 <= size.width) periods.add(x0 to x1)
                            m++
                        }
                        if (periods.isEmpty()) continue
                        val (x0, x1) = periods[pulse.crest % periods.size]
                        val ghostColor = lerp(
                            color.copy(alpha = 0.4f * env),
                            tint.copy(alpha = 0.4f * env),
                            0.55f,
                        )
                        drawPath(
                            wavePath(size.width, size.height / 2f,
                                amplitude = amp * (1f + pulse.bulge * env),
                                wavelength = wl,
                                phase = phase,
                                fromX = x0,
                                toX = x1),
                            ghostColor,
                            style = Stroke(width = 3.dp.toPx()),
                        )
                    }
                    drawPath(
                        wavePath(size.width, size.height / 2f,
                            amplitude = amp,
                            wavelength = wl,
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
                        if (serverless) {
                            "the family rotated this phone out — it was removed. " +
                                "Its keys are useless now."
                        } else {
                            "the relay no longer knows this phone — it was removed " +
                                "from the family. Its keys are useless now."
                        },
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
                if (serverless && status.running && !online && !connectingGrace) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "no peers on the lan — devices find each other on the " +
                            "same network; is the laptop awake?",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                }
                if (status.protoMismatch.isNotEmpty()) {
                    Spacer(Modifier.height(6.dp))
                    val versions = status.protoMismatch.values.distinct().joinToString { "v$it" }
                    Text(
                        "a peer speaks protocol $versions, this phone speaks " +
                            "v${dev.murmur.core.Envelope.VERSION} — they can't understand " +
                            "each other; update whichever is behind",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                        textAlign = TextAlign.Center,
                    )
                }
                if (status.portBusy) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "the lan port is busy — another copy of murmur on this " +
                            "phone? two copies steal each other's links; uninstall one",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                        textAlign = TextAlign.Center,
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


/** One pulse's full leap, milliseconds. */
private const val PULSE_MS = 1400L

/** A wave-packet ghost: when it was born, what kind tore it off, and
 *  its rolled-once shape — lift in dp, span as a fraction of width. */
private data class GhostPulse(
    val t0: Long,
    val kind: PulseKind,
    /** Which visible period to haunt (modulo however many fit). */
    val crest: Int,
    /** How far past the baseline amplitude the crest swells at peak. */
    val bulge: Float,
)

// Shades of green — one family, distinguishable kinds.
private val PULSE_CLIP = Color(0xFFB8BB26)   // bright gruvbox green
private val PULSE_NOTIFY = Color(0xFF8EC07C) // aqua-green
private val PULSE_MEDIA = Color(0xFF98971A)  // deep olive
