package dev.murmur.ui.widgets

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Shield
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner

/**
 * Everything murmur can't do for itself, as two-line grant rows. The
 * card vanishes once nothing is missing. POST_NOTIFICATIONS is the one
 * permission Android lets us ask for directly, so we ask on sight.
 */
@Composable
fun PermissionsCard() {
    val ctx = LocalContext.current
    var listener by remember { mutableStateOf(true) }
    var battery by remember { mutableStateOf(true) }
    var postNotif by remember { mutableStateOf(true) }

    fun recheck() {
        listener = NotificationManagerCompat
            .getEnabledListenerPackages(ctx).contains(ctx.packageName)
        val pm = ctx.getSystemService(android.content.Context.POWER_SERVICE)
            as android.os.PowerManager
        battery = pm.isIgnoringBatteryOptimizations(ctx.packageName)
        postNotif = Build.VERSION.SDK_INT < 33 ||
            ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
    }

    // Re-check whenever the user returns from a settings screen.
    val owner = LocalLifecycleOwner.current
    DisposableEffect(owner) {
        val obs = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_RESUME) recheck()
        }
        owner.lifecycle.addObserver(obs)
        recheck()
        onDispose { owner.lifecycle.removeObserver(obs) }
    }

    val postNotifLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { recheck() }
    // The system dialog — shown once per process while still missing.
    var asked by remember { mutableStateOf(false) }
    LaunchedEffect(postNotif) {
        if (!postNotif && !asked) {
            asked = true
            postNotifLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    if (listener && battery && postNotif) return

    FeatureCard("permissions", Icons.Outlined.Shield) {
        Text(
            "murmur needs a hand with a few switches:",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (!listener) {
            PermRow(
                title = "notification access",
                hint = "mirrors your notifications and media to laptops",
            ) {
                ctx.startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
            }
        }
        if (!postNotif) {
            PermRow(
                title = "app notifications",
                hint = "shows the relay status in the shade",
            ) {
                postNotifLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        }
        if (!battery) {
            PermRow(
                title = "battery exemption",
                hint = "keeps murmur connected in the background",
            ) {
                ctx.startActivity(
                    Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                        .setData(android.net.Uri.parse("package:${ctx.packageName}")),
                )
            }
        }
    }
}

@Composable
private fun PermRow(title: String, hint: String, onGrant: () -> Unit) {
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(
                hint,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        TextButton(onClick = onGrant) { Text("grant") }
    }
}
