package dev.murmur.ui.widgets

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Lightbulb
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.foundation.layout.Row
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.murmur.core.Secrets

/**
 * The first minutes after pairing — when features either sell
 * themselves or never get discovered. One dismissible card with three
 * things to try right now; once dismissed it never comes back.
 */
@Composable
fun FirstRunHint() {
    val ctx = LocalContext.current
    val settled by Secrets.settledFlow.collectAsState()
    val prefs = remember { ctx.getSharedPreferences("murmur_ui", android.content.Context.MODE_PRIVATE) }
    var dismissed by remember { mutableStateOf(prefs.getBoolean("firstrun_dismissed", false)) }

    AnimatedVisibility(
        visible = settled && !dismissed,
        enter = fadeIn(tween(800)) + expandVertically(tween(800)),
        exit = fadeOut(tween(300)) + shrinkVertically(tween(300)),
    ) {
        FeatureCard("try it now", Icons.Outlined.Lightbulb) {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                HintLine("copy something on a laptop — it lands in this phone's clipboard")
                HintLine("notifications already mirror — watch the laptop when one arrives")
                HintLine("select text anywhere → the ⋮ menu → murmur sends it to the laptop")
                Spacer(Modifier.height(2.dp))
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.End,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        "got it",
                        style = MaterialTheme.typography.labelLarge,
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.padding(end = 4.dp),
                    )
                    IconButton(onClick = {
                        prefs.edit().putBoolean("firstrun_dismissed", true).apply()
                        dismissed = true
                    }) {
                        Icon(Icons.Outlined.Close, contentDescription = "dismiss")
                    }
                }
            }
        }
    }
}

@Composable
private fun HintLine(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            "·",
            style = MaterialTheme.typography.titleMedium,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(end = 10.dp),
        )
        Text(
            text,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
