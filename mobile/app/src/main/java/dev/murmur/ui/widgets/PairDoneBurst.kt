package dev.murmur.ui.widgets

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.util.lerp

private val BackOut = CubicBezierEasing(0.34f, 1.56f, 0.64f, 1f)
private const val DURATION = 2800

/**
 * One-shot celebration when the phone joins the family — built around
 * the murmur mark itself. The whisper wave draws itself outward from the
 * center, fires two echoes that travel to the edges like a whisper
 * spreading, then settles into a gentle breath while the greeting lands.
 * [onFinished] fires once at the end — the caller swaps in the dashboard.
 */
@Composable
fun PairDoneBurst(onFinished: () -> Unit = {}) {
    val t = remember { Animatable(0f) }
    LaunchedEffect(Unit) {
        t.animateTo(1f, tween(DURATION, easing = LinearEasing))
        onFinished()
    }
    val primary = MaterialTheme.colorScheme.primary
    val secondary = MaterialTheme.colorScheme.secondary
    val tertiary = MaterialTheme.colorScheme.tertiary
    val echoColors = listOf(secondary, tertiary)

    Box(Modifier.fillMaxWidth().height(230.dp), contentAlignment = Alignment.Center) {
        Canvas(Modifier.matchParentSize()) {
            val cx = size.width / 2f
            val cy = size.height * 0.42f
            val wavelength = size.width / 3.2f
            val maxAmp = 26.dp.toPx()
            val stroke = Stroke(width = 5.dp.toPx())

            // Main wave — reveals outward from the center, amplitude pops.
            val reveal = (t.value / 0.30f).coerceIn(0f, 1f)
            if (reveal > 0f) {
                val amp = maxAmp * BackOut.transform((t.value / 0.42f).coerceIn(0f, 1f)) *
                    // settles into a slow breath for the rest of the run
                    (1f + 0.18f * kotlin.math.sin(t.value * 9f) * t.value)
                val half = size.width * reveal / 2f
                drawPath(
                    wavePath(size.width, cy, amp, wavelength,
                        fromX = cx - half, toX = cx + half),
                    primary, style = stroke,
                )
            }

            // Echo slices racing to both edges — the whisper propagating.
            for (i in echoColors.indices) {
                val p = ((t.value - 0.28f - i * 0.13f) / 0.58f).coerceIn(0f, 1f)
                if (p <= 0f || p >= 1f) continue
                val amp = maxAmp * 0.7f * (1f - p * 0.6f)
                val slice = size.width * lerp(0.22f, 0.34f, p)
                for (side in intArrayOf(-1, 1)) {
                    val xc = cx + side * size.width * 0.42f * p
                    drawPath(
                        wavePath(size.width, cy, amp, wavelength,
                            phase = side * 1.7f,
                            fromX = xc - slice / 2f, toX = xc + slice / 2f),
                        echoColors[i], alpha = (1f - p) * 0.7f,
                        style = Stroke(width = lerp(5f, 1.5f, p).dp.toPx()),
                    )
                }
            }
        }

        // Greeting fades in once the wave has spoken.
        val greet = ((t.value - 0.52f) / 0.30f).coerceIn(0f, 1f)
        Text(
            "welcome to the family",
            style = MaterialTheme.typography.headlineSmall,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .graphicsLayer {
                    alpha = greet
                    translationY = (1f - greet) * 28f
                },
        )
    }
}
