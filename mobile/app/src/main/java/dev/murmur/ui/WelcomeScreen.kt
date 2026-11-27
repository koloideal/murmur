package dev.murmur.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.murmur.ui.theme.Unbounded
import dev.murmur.ui.widgets.wavePath

/**
 * The very first screen the app ever shows — before any permission
 * ask, before the family-kind question. One job: the user should know
 * what murmur is and what it will ask of them *before* Android starts
 * throwing system dialogs in their face. The wave is the same mark as
 * the launcher icon, breathing at three depths so the screen is alive
 * without a single asset.
 */
@Composable
fun WelcomeScreen(onContinue: () -> Unit) {
    val transition = rememberInfiniteTransition(label = "welcome")
    val phase by transition.animateFloat(
        initialValue = 0f,
        targetValue = 6.2832f,
        animationSpec = infiniteRepeatable(tween(3000, easing = LinearEasing), RepeatMode.Restart),
        label = "phase",
    )
    val phase2 by transition.animateFloat(
        initialValue = 0f,
        targetValue = 6.2832f,
        animationSpec = infiniteRepeatable(tween(4200, easing = LinearEasing), RepeatMode.Restart),
        label = "phase2",
    )
    val phase3 by transition.animateFloat(
        initialValue = 0f,
        targetValue = 6.2832f,
        animationSpec = infiniteRepeatable(tween(5600, easing = LinearEasing), RepeatMode.Restart),
        label = "phase3",
    )

    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            Modifier.fillMaxSize().padding(32.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spacer(Modifier.weight(1f))
            Text(
                "welcome to",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                "murmur",
                style = MaterialTheme.typography.displayLarge,
                fontFamily = Unbounded,
                color = MaterialTheme.colorScheme.primary,
            )
            Spacer(Modifier.height(18.dp))
            // Three waves out of phase — the family talking.
            val waveColor = MaterialTheme.colorScheme.primary
            Canvas(Modifier.fillMaxWidth().height(72.dp)) {
                val mid = size.height / 2f
                listOf(
                    Triple(phase, 0.95f, 4.dp),
                    Triple(phase2, 0.55f, 3.dp),
                    Triple(phase3, 0.30f, 2.dp),
                ).forEach { (ph, alpha, w) ->
                    drawPath(
                        wavePath(
                            size.width, mid,
                            amplitude = 16.dp.toPx() * alpha,
                            wavelength = size.width / 2.6f,
                            phase = ph,
                        ),
                        waveColor.copy(alpha = alpha),
                        style = Stroke(width = w.toPx(), cap = StrokeCap.Round),
                    )
                }
            }
            Spacer(Modifier.height(18.dp))
            Text(
                "your phone and your laptops, one encrypted family —\n" +
                    "clipboard, notifications and media whispered " +
                    "directly between your devices",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.weight(1f))
            Text(
                "first, android needs a few permissions — next screen explains each",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(12.dp))
            Button(onClick = onContinue, modifier = Modifier.fillMaxWidth()) {
                Text("continue")
            }
        }
    }
}
