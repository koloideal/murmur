package dev.murmur.ui.widgets

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.util.lerp

private val BackOut = CubicBezierEasing(0.34f, 1.56f, 0.64f, 1f)

/**
 * One-shot celebration when the phone joins the family: concentric
 * "whisper" rings radiate out (the murmur wave), a core pops in with
 * overshoot and a check lands on it. ~2.4s, then the caller settles.
 */
@Composable
fun PairDoneBurst(modifier: Modifier = Modifier) {
    val t = remember { Animatable(0f) }
    LaunchedEffect(Unit) {
        t.animateTo(1f, tween(2400))
    }
    val primary = MaterialTheme.colorScheme.primary
    val secondary = MaterialTheme.colorScheme.secondary
    val tertiary = MaterialTheme.colorScheme.tertiary
    val onPrimary = MaterialTheme.colorScheme.onPrimary

    val ringColors = listOf(primary, secondary, tertiary, primary)

    Box(modifier.fillMaxWidth().height(220.dp), contentAlignment = Alignment.Center) {
        Canvas(Modifier.matchParentSize()) {
            val center = Offset(size.width / 2f, size.height / 2f)

            // Expanding rings — staggered, fading as they travel outward.
            for (i in ringColors.indices) {
                val p = ((t.value - i * 0.14f) / 0.62f).coerceIn(0f, 1f)
                if (p <= 0f) continue
                drawCircle(
                    color = ringColors[i],
                    alpha = (1f - p) * 0.55f,
                    radius = lerp(36f, size.height * 0.52f, p),
                    center = center,
                    style = Stroke(width = lerp(7f, 1.5f, p)),
                )
            }

            // Core — pops in with overshoot during the first third.
            val pop = BackOut.transform((t.value / 0.38f).coerceIn(0f, 1f))
            if (pop > 0f) {
                drawCircle(primary, radius = 30.dp.toPx() * pop, center = center)
                // Faint halo breathing behind the core until the end.
                drawCircle(
                    secondary,
                    alpha = 0.18f * (1f - t.value * 0.5f),
                    radius = 30.dp.toPx() * pop * (1f + t.value * 0.5f),
                    center = center,
                )
            }
        }

        // Check rides the same pop curve.
        val pop = BackOut.transform((t.value / 0.38f).coerceIn(0f, 1f))
        Icon(
            Icons.Filled.Check,
            contentDescription = null,
            tint = onPrimary,
            modifier = Modifier
                .size(30.dp)
                .graphicsLayer {
                    scaleX = pop
                    scaleY = pop
                    alpha = pop.coerceIn(0f, 1f)
                },
        )
    }
}
