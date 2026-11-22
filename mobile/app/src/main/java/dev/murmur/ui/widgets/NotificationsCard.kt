package dev.murmur.ui.widgets

import android.content.pm.PackageManager
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.NotificationsOff
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.core.graphics.drawable.toBitmap
import dev.murmur.bridge.NotificationBridge
import dev.murmur.core.Secrets

/**
 * What leaves the phone: a "hide message text" switch and a picker of
 * the apps that have posted notifications — tap one to keep it local.
 * Muted apps sit first and are struck through, so the state reads at a
 * glance without a legend.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun NotificationsCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val settled by Secrets.settledFlow.collectAsState()
    val removed by Secrets.removedFlow.collectAsState()
    val muted by Secrets.mutedFlow.collectAsState()
    val hide by Secrets.hideTextFlow.collectAsState()
    val seen by NotificationBridge.apps.collectAsState()

    val pm = ctx.packageManager
    fun label(pkg: String) = seen[pkg]?.label ?: runCatching {
        pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    }.getOrNull() ?: pkg.substringAfterLast('.')

    val entries = remember(seen, muted) {
        val others = seen.values
            .filter { it.pkg !in muted }
            .sortedByDescending { it.seen }
            .take(MAX_RECENT)
            .map { it.pkg }
        muted.sortedBy(::label) + others
    }

    AnimatedVisibility(
        visible = settled && !removed,
        enter = fadeIn(tween(600)) + expandVertically(tween(600)),
    ) {
        FeatureCard("notifications", Icons.Outlined.Notifications) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("hide message text", style = MaterialTheme.typography.bodyLarge)
                    Text(
                        "laptops show only the app and the title",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Switch(checked = hide, onCheckedChange = { secrets.hideNotificationText = it })
            }
            Spacer(Modifier.height(18.dp))
            Text("muted apps", style = MaterialTheme.typography.titleSmall)
            Text(
                if (entries.isEmpty()) "apps show up here once they post a notification"
                else "tap an app to keep its notifications on the phone",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (entries.isNotEmpty()) {
                Spacer(Modifier.height(10.dp))
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    entries.forEach { pkg ->
                        val isMuted = pkg in muted
                        FilterChip(
                            selected = isMuted,
                            onClick = {
                                secrets.mutedApps = if (isMuted) muted - pkg else muted + pkg
                            },
                            label = {
                                Text(
                                    label(pkg),
                                    textDecoration = if (isMuted) TextDecoration.LineThrough else null,
                                )
                            },
                            leadingIcon = {
                                if (isMuted) {
                                    Icon(
                                        Icons.Outlined.NotificationsOff,
                                        contentDescription = "muted",
                                        modifier = Modifier.size(FilterChipDefaults.IconSize),
                                    )
                                } else {
                                    AppIcon(pm, pkg)
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun AppIcon(pm: PackageManager, pkg: String) {
    val icon: ImageBitmap? = remember(pkg) {
        runCatching { pm.getApplicationIcon(pkg).toBitmap(64, 64).asImageBitmap() }.getOrNull()
    }
    if (icon != null) {
        Image(icon, contentDescription = null, modifier = Modifier.size(FilterChipDefaults.IconSize))
    } else {
        Spacer(Modifier.width(0.dp))
    }
}

private const val MAX_RECENT = 16
