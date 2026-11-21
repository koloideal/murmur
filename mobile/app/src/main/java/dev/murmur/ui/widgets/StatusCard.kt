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
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Hub
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
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
 * whisper wave, identity details underneath. Hidden until the phone is
 * paired and the welcome burst has finished ([Secrets.settledFlow]).
 */
@Composable
fun StatusCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val status by MurmurService.status.collectAsState()
    val settled by Secrets.settledFlow.collectAsState()

    val (color, label) = when (status.relay) {
        Relay.State.ONLINE -> MaterialTheme.colorScheme.secondary to "online"
        Relay.State.CONNECTING -> MaterialTheme.colorScheme.primary to "connecting"
        Relay.State.OFFLINE -> MaterialTheme.colorScheme.error to "offline"
    }

    // The wave breathes while the relay is online.
    val phase by if (status.relay == Relay.State.ONLINE) {
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
                if (status.relay == Relay.State.OFFLINE) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "can't reach the relay — is it running?",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.height(16.dp))
                StatusRow("device", secrets.deviceId ?: "—", dim = secrets.deviceId == null)
                StatusRow("server", secrets.serverUrl.ifEmpty { "—" })
            }
        }
    }
}
