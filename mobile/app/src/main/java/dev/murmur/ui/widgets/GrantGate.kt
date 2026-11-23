package dev.murmur.ui.widgets

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Shield
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
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
import kotlinx.coroutines.delay

/**
 * Access murmur literally cannot function without — asked the moment
 * the app opens, before anything else renders. Until all three are
 * granted the gate IS the app: grant them or quit.
 */
class Grants(
    val listener: Boolean,
    val battery: Boolean,
    val postNotif: Boolean,
) {
    val all get() = listener && battery && postNotif
}

private fun Context.readGrants() = Grants(
    listener = NotificationManagerCompat
        .getEnabledListenerPackages(this).contains(packageName),
    battery = (getSystemService(Context.POWER_SERVICE) as PowerManager)
        .isIgnoringBatteryOptimizations(packageName),
    postNotif = Build.VERSION.SDK_INT < 33 ||
        ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) ==
        PackageManager.PERMISSION_GRANTED,
)

/** Live grant state — re-read on every resume and polled while
 *  anything's missing (settings screens lag before reporting). */
@Composable
fun rememberGrants(): Grants {
    val ctx = LocalContext.current
    var grants by remember { mutableStateOf(ctx.readGrants()) }
    val owner = LocalLifecycleOwner.current
    DisposableEffect(owner) {
        val obs = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_RESUME) grants = ctx.readGrants()
        }
        owner.lifecycle.addObserver(obs)
        grants = ctx.readGrants()
        onDispose { owner.lifecycle.removeObserver(obs) }
    }
    LaunchedEffect(grants.all) {
        while (!grants.all) {
            delay(1500)
            grants = ctx.readGrants()
        }
    }
    return grants
}

@Composable
fun GrantGate(grants: Grants) {
    val ctx = LocalContext.current
    val activity = ctx as? Activity

    // Runtime permissions fire the instant the app opens. Camera rides
    // along for the QR path — a denied camera only means "type the
    // invite by hand", so it isn't a gate condition.
    val runtime = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { }
    var asked by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        if (!asked) {
            asked = true
            runtime.launch(
                buildList {
                    if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
                    add(Manifest.permission.CAMERA)
                }.toTypedArray(),
            )
        }
    }

    // A bare Column leaves LocalContentColor black — unreadable on
    // the gruvbox background. Surface owns both background and the
    // default text color for everything inside.
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            Modifier
                .fillMaxSize()
                .padding(horizontal = 28.dp),
            verticalArrangement = Arrangement.Center,
        ) {
        Icon(
            Icons.Outlined.Shield,
            contentDescription = null,
            modifier = Modifier.size(44.dp),
            tint = MaterialTheme.colorScheme.primary,
        )
        Spacer(Modifier.height(16.dp))
        Text(
            "murmur can't run without these",
            style = MaterialTheme.typography.headlineSmall,
        )
        Spacer(Modifier.height(6.dp))
        Text(
            "it lives in the background moving your notifications — " +
                "no access, no app",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(28.dp))
        if (!grants.listener) {
            GrantRow(
                title = "notification access",
                hint = "sees notifications to mirror them",
            ) {
                ctx.startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
            }
        }
        if (!grants.postNotif) {
            GrantRow(
                title = "app notifications",
                hint = "keeps the status notification alive",
            ) {
                ctx.startActivity(
                    Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                        .putExtra(Settings.EXTRA_APP_PACKAGE, ctx.packageName),
                )
            }
        }
        if (!grants.battery) {
            GrantRow(
                title = "battery exemption",
                hint = "keeps the link alive in the background",
            ) {
                ctx.startActivity(
                    Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                        .setData(android.net.Uri.parse("package:${ctx.packageName}")),
                )
            }
        }
        Spacer(Modifier.height(36.dp))
        TextButton(
            onClick = { activity?.finish() },
            modifier = Modifier.fillMaxWidth(),
        ) { Text("quit murmur") }
        }
    }
}

@Composable
private fun GrantRow(title: String, hint: String, onGrant: () -> Unit) {
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
        OutlinedButton(
            onClick = onGrant,
            shape = MaterialTheme.shapes.small,
            contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
        ) { Text("grant") }
    }
}
