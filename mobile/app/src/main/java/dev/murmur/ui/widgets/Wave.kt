package dev.murmur.ui.widgets

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import kotlin.math.PI
import kotlin.math.sin

/**
 * The murmur mark — the same whisper wave as the launcher icon
 * (`ic_launcher_foreground`), parameterised so cards and animations can
 * draw it at any width, amplitude and phase. A sine looks identical to
 * the icon's quadratic bumps at these sizes.
 */
fun wavePath(
    width: Float,
    centerY: Float,
    amplitude: Float,
    wavelength: Float,
    phase: Float = 0f,
    fromX: Float = 0f,
    toX: Float = width,
    offsetY: Float = 0f,
    ampWindow: (Float) -> Float = { 1f },
): Path {
    val p = Path()
    var x = fromX
    p.moveTo(x, centerY + offsetY + amplitude * ampWindow(x / width) * sin(TAU * x / wavelength + phase))
    while (x < toX) {
        x = (x + STEP).coerceAtMost(toX)
        p.lineTo(x, centerY + offsetY + amplitude * ampWindow(x / width) * sin(TAU * x / wavelength + phase))
    }
    return p
}

/** Sampled crest positions — for particles riding the wave. */
fun wavePoint(
    x: Float,
    centerY: Float,
    amplitude: Float,
    wavelength: Float,
    phase: Float,
): Offset = Offset(x, centerY + amplitude * sin(TAU * x / wavelength + phase))

private const val TAU = (2f * PI).toFloat()
private const val STEP = 3f
