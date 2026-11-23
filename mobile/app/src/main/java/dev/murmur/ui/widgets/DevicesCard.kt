package dev.murmur.ui.widgets

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Devices
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import dev.murmur.MurmurService
import dev.murmur.core.Secrets

/**
 * The rest of the family as the relay sees it — name, live dot, ping —
 * refreshed by the service's ListDevices poller. Self is excluded.
 */
@Composable
fun DevicesCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val selfId = remember { secrets.deviceId }
    val status by MurmurService.status.collectAsState()
    val settled by Secrets.settledFlow.collectAsState()
    val removed by Secrets.removedFlow.collectAsState()

    val others = status.devices.filter { it.device_id != selfId && !it.revoked }
    // The device list hides self by design, but then an all-offline
    // roster reads like the phone itself is down — show "me" up top.
    val me = status.devices.firstOrNull { it.device_id == selfId }

    AnimatedVisibility(
        visible = settled && !removed,
        enter = fadeIn(tween(600)) + expandVertically(tween(600)),
    ) {
        FeatureCard("family", Icons.Outlined.Devices) {
            me?.let { d ->
                Row(
                    Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier
                            .size(8.dp)
                            .background(
                                if (d.online) MaterialTheme.colorScheme.secondary
                                else MaterialTheme.colorScheme.onSurfaceVariant,
                                MaterialTheme.shapes.small,
                            ),
                    )
                    Spacer(Modifier.width(10.dp))
                    Text(
                        "${d.name} (me)",
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurface,
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        if (d.online) "online" else "offline",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            if (others.isEmpty()) {
                Text(
                    "no other devices yet — pair a laptop with murmurctl invite",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                others.forEach { d ->
                    Row(
                        Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Box(
                            Modifier
                                .size(8.dp)
                                .background(
                                    if (d.online) MaterialTheme.colorScheme.secondary
                                    else MaterialTheme.colorScheme.onSurfaceVariant,
                                    MaterialTheme.shapes.small,
                                ),
                        )
                        Spacer(Modifier.width(10.dp))
                        Text(
                            d.name,
                            style = MaterialTheme.typography.bodyLarge,
                            color = if (d.online) MaterialTheme.colorScheme.onSurface
                            else MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.weight(1f),
                        )
                        Text(
                            buildAnnotatedString {
                                when {
                                    !d.online -> append("offline")
                                    // Phones get a bare status — a relay
                                    // RTT or link route means nothing on
                                    // a handset row.
                                    secrets.roster[d.device_id]?.second == "phone" ->
                                        append("online")
                                    else -> {
                                        val lanRtt = status.lanPeers[d.device_id]
                                        append(
                                            when {
                                                lanRtt != null && lanRtt > 0 -> "$lanRtt ms"
                                                lanRtt != null -> "online"
                                                d.rtt_ms != null -> "${d.rtt_ms} ms"
                                                else -> "online"
                                            },
                                        )
                                        // No route words in a serverless
                                        // family — lan is the only road.
                                        if (status.familyKind != "serverless") {
                                            withStyle(
                                                SpanStyle(color = MaterialTheme.colorScheme.outline),
                                            ) {
                                                append(if (lanRtt != null) " · on lan" else " · on wan")
                                            }
                                        }
                                    }
                                }
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}
