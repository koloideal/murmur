package dev.murmur.ui.widgets

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Key
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver

/** The two system permissions the agent can't grant itself. */
@Composable
fun PermissionsCard() {
    val ctx = LocalContext.current
    var notif by remember { mutableStateOf(hasNotificationAccess(ctx)) }
    var battery by remember { mutableStateOf(ignoresBattery(ctx)) }

    // Re-check when the user comes back from a settings screen.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val obs = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_RESUME) {
                notif = hasNotificationAccess(ctx)
                battery = ignoresBattery(ctx)
            }
        }
        lifecycle.addObserver(obs)
        onDispose { lifecycle.removeObserver(obs) }
    }

    FeatureCard("permissions", Icons.Outlined.Key) {
        PermissionRow(
            granted = notif,
            label = "notification access",
            hint = "notifications & media",
        ) {
            ctx.startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
        }
        PermissionRow(
            granted = battery,
            label = "battery exemption",
            hint = "stay connected in background",
        ) {
            ctx.startActivity(
                Intent(
                    Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                    Uri.parse("package:${ctx.packageName}"),
                ),
            )
        }
    }
}

@Composable
private fun PermissionRow(
    granted: Boolean,
    label: String,
    hint: String,
    onFix: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                if (granted) Icons.Outlined.CheckCircle else Icons.Outlined.WarningAmber,
                contentDescription = null,
                tint = if (granted) MaterialTheme.colorScheme.primary
                else MaterialTheme.colorScheme.error,
            )
            Spacer(Modifier.width(10.dp))
            Text(label, style = MaterialTheme.typography.bodyMedium)
        }
        if (!granted) {
            TextButton(onClick = onFix) { Text("grant") }
        } else {
            Text(hint, style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

private fun hasNotificationAccess(ctx: Context): Boolean {
    val enabled = Settings.Secure.getString(
        ctx.contentResolver,
        "enabled_notification_listeners",
    ) ?: return false
    return enabled.contains(ctx.packageName)
}

private fun ignoresBattery(ctx: Context): Boolean =
    ctx.getSystemService(PowerManager::class.java)
        .isIgnoringBatteryOptimizations(ctx.packageName)
