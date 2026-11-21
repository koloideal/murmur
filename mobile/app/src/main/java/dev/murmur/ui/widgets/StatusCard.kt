package dev.murmur.ui.widgets

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Hub
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.murmur.MurmurService
import dev.murmur.core.Relay
import dev.murmur.core.Secrets

/** Connection + identity at a glance. Hidden until the phone is paired —
 *  before that there's nothing to report and the pairing card leads. */
@Composable
fun StatusCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val status by MurmurService.status.collectAsState()

    if (secrets.deviceId == null) return

    FeatureCard("connection", Icons.Outlined.Hub) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val (color, label) = when (status.relay) {
                Relay.State.ONLINE -> Color(0xFF4CAF50) to "online"
                Relay.State.CONNECTING -> Color(0xFFFFC107) to "connecting…"
                Relay.State.OFFLINE -> Color(0xFFE57373) to "offline"
            }
            Box(Modifier.size(10.dp).background(color, MaterialTheme.shapes.small))
            Spacer(Modifier.width(8.dp))
            Text(label, style = MaterialTheme.typography.bodyLarge)
        }
        StatusRow("device", secrets.deviceId ?: "—", dim = secrets.deviceId == null)
        StatusRow("server", secrets.serverUrl.ifEmpty { "—" })
    }
}
