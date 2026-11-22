package dev.murmur.ui.widgets

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.FlashlightOff
import androidx.compose.material.icons.outlined.FlashlightOn
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.BarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory

/**
 * Full-screen QR-only scanner: live camera, a square rounded viewfinder
 * in the app's accent, torch toggle. Decodes once, then hands the text
 * back — no laser line, no landscape lock, nothing barcode-shaped.
 */
@Composable
fun QrScanner(onResult: (String) -> Unit, onDismiss: () -> Unit) {
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(
            usePlatformDefaultWidth = false,
            decorFitsSystemWindows = false,
        ),
    ) {
        val ctx = LocalContext.current
        val haptics = LocalHapticFeedback.current
        val lifecycle = LocalLifecycleOwner.current.lifecycle
        var torch by remember { mutableStateOf(false) }
        val view = remember {
            BarcodeView(ctx).apply {
                decoderFactory = DefaultDecoderFactory(listOf(BarcodeFormat.QR_CODE))
            }
        }

        DisposableEffect(lifecycle) {
            val obs = LifecycleEventObserver { _, e ->
                when (e) {
                    Lifecycle.Event.ON_RESUME -> view.resume()
                    Lifecycle.Event.ON_PAUSE -> view.pause()
                    else -> Unit
                }
            }
            lifecycle.addObserver(obs)
            view.resume()
            view.decodeSingle { r ->
                r.text?.takeIf { it.isNotBlank() }?.let {
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    onResult(it)
                }
            }
            onDispose {
                lifecycle.removeObserver(obs)
                view.pause()
            }
        }

        val accent = MaterialTheme.colorScheme.primary
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            AndroidView({ view }, Modifier.fillMaxSize())

            Canvas(Modifier.fillMaxSize()) {
                val side = size.minDimension * 0.7f
                val left = (size.width - side) / 2f
                val top = (size.height - side) / 2f
                val r = 28.dp.toPx()
                val hole = RoundRect(left, top, left + side, top + side, CornerRadius(r))
                drawPath(
                    Path().apply {
                        fillType = PathFillType.EvenOdd
                        addRect(androidx.compose.ui.geometry.Rect(Offset.Zero, size))
                        addRoundRect(hole)
                    },
                    Color.Black.copy(alpha = 0.62f),
                )
                // Four accent corner brackets hugging the rounded hole.
                val arm = side * 0.16f
                val stroke = Stroke(width = 5.dp.toPx(), cap = StrokeCap.Round)
                val d = r * 2
                listOf(
                    Offset(left, top) to 180f,
                    Offset(left + side - d, top) to 270f,
                    Offset(left + side - d, top + side - d) to 0f,
                    Offset(left, top + side - d) to 90f,
                ).forEach { (o, start) ->
                    drawArc(accent, start, 90f, false, o, Size(d, d), style = stroke)
                }
                val l = left; val t = top; val rr = left + side; val b = top + side
                listOf(
                    Offset(l + r, t) to Offset(l + r + arm, t),
                    Offset(l, t + r) to Offset(l, t + r + arm),
                    Offset(rr - r, t) to Offset(rr - r - arm, t),
                    Offset(rr, t + r) to Offset(rr, t + r + arm),
                    Offset(l + r, b) to Offset(l + r + arm, b),
                    Offset(l, b - r) to Offset(l, b - r - arm),
                    Offset(rr - r, b) to Offset(rr - r - arm, b),
                    Offset(rr, b - r) to Offset(rr, b - r - arm),
                ).forEach { (a, z) ->
                    drawLine(accent, a, z, stroke.width, StrokeCap.Round)
                }
            }

            Column(
                Modifier.fillMaxSize().safeDrawingPadding().padding(24.dp),
                verticalArrangement = Arrangement.SpaceBetween,
            ) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    FilledTonalIconButton(onClick = onDismiss) {
                        Icon(Icons.Outlined.Close, contentDescription = "close scanner")
                    }
                }
                Column(
                    Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(
                        "point at the QR from",
                        style = MaterialTheme.typography.bodyLarge,
                        color = Color.White,
                        textAlign = TextAlign.Center,
                    )
                    CommandLine("murmurctl invite", Modifier.padding(top = 8.dp))
                    FilledTonalIconButton(
                        onClick = {
                            torch = !torch
                            view.setTorch(torch)
                        },
                        modifier = Modifier.padding(top = 20.dp),
                    ) {
                        Icon(
                            if (torch) Icons.Outlined.FlashlightOn else Icons.Outlined.FlashlightOff,
                            contentDescription = "torch",
                        )
                    }
                }
            }
        }
    }
}
