package dev.murmur.ui.widgets

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp

/** A terminal command as a chip: dim `$`, monospace body. */
@Composable
fun CommandLine(cmd: String, modifier: Modifier = Modifier) {
    Text(
        buildAnnotatedString {
            withStyle(SpanStyle(color = MaterialTheme.colorScheme.onSurfaceVariant)) { append("$ ") }
            append(cmd)
        },
        style = MaterialTheme.typography.bodyMedium,
        fontFamily = FontFamily.Monospace,
        color = MaterialTheme.colorScheme.onSurface,
        modifier = modifier
            .background(MaterialTheme.colorScheme.surfaceContainerHigh, MaterialTheme.shapes.small)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

/** One numbered step of a "do this on the laptop" walkthrough. */
class Step(val text: String, val cmd: String? = null)

/** Numbered steps; the current one is lit in the accent, done ones fade. */
@Composable
fun StepList(steps: List<Step>, current: Int) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        steps.forEachIndexed { i, s ->
            val lit = i == current
            val done = i < current
            Row(verticalAlignment = Alignment.Top) {
                Box(
                    Modifier
                        .size(24.dp)
                        .background(
                            if (lit) MaterialTheme.colorScheme.primary
                            else MaterialTheme.colorScheme.surfaceContainerHigh,
                            CircleShape,
                        ),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (done) "✓" else "${i + 1}",
                        style = MaterialTheme.typography.labelMedium,
                        color = if (lit) MaterialTheme.colorScheme.onPrimary
                        else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Column(Modifier.padding(start = 12.dp).fillMaxWidth()) {
                    Text(
                        s.text,
                        style = MaterialTheme.typography.bodyMedium,
                        color = if (lit) MaterialTheme.colorScheme.onSurface
                        else MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                    s.cmd?.let { CommandLine(it, Modifier.padding(top = 6.dp)) }
                }
            }
        }
    }
}
