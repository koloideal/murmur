package dev.murmur.ui.widgets

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.murmur.core.Secrets

/**
 * What leaves the phone: a master mirror switch, and — only while it's
 * on — "hide message text".
 */
@Composable
fun NotificationsCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val settled by Secrets.settledFlow.collectAsState()
    val removed by Secrets.removedFlow.collectAsState()
    val hide by Secrets.hideTextFlow.collectAsState()
    val mirror by Secrets.mirrorFlow.collectAsState()
    val dismissRemote by Secrets.dismissRemoteFlow.collectAsState()

    AnimatedVisibility(
        visible = settled && !removed,
        enter = fadeIn(tween(600)) + expandVertically(tween(600)),
    ) {
        FeatureCard("notifications", Icons.Outlined.Notifications) {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ToggleRow(
                    "mirror notifications",
                    "show this phone's notifications on laptops",
                    checked = mirror,
                    onChange = { secrets.mirrorNotifications = it },
                )
                AnimatedVisibility(
                    visible = mirror,
                    enter = fadeIn(tween(250)) + expandVertically(tween(250)),
                    exit = fadeOut(tween(200)) + shrinkVertically(tween(200)),
                ) {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        ToggleRow(
                            "hide message text",
                            "laptops show only the app and the title",
                            checked = hide,
                            onChange = { secrets.hideNotificationText = it },
                        )
                        ToggleRow(
                            "dismiss everywhere",
                            "closing a notification on a laptop clears it here too",
                            checked = dismissRemote,
                            onChange = { secrets.dismissRemote = it },
                        )
                    }
                }
            }
        }
    }
}

/** One switch on a tonal row — off-state tracks blend into the card,
 *  so the row needs its own surface to read as a control. */
@Composable
fun ToggleRow(title: String, hint: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .border(
                1.dp,
                MaterialTheme.colorScheme.outlineVariant,
                MaterialTheme.shapes.small,
            )
            .padding(start = 16.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(
                hint,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Spacer(Modifier.width(12.dp))
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            colors = SwitchDefaults.colors(
                uncheckedThumbColor = MaterialTheme.colorScheme.outline,
                uncheckedTrackColor = MaterialTheme.colorScheme.surfaceContainerLowest,
                uncheckedBorderColor = MaterialTheme.colorScheme.outline,
            ),
        )
    }
}
