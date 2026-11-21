package dev.murmur.ui.widgets

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.murmur.core.Secrets

/**
 * Home-screen widget registry. A feature's UI is a self-contained card
 * composable that owns its state — add a feature to the home screen by
 * writing the card file and appending one line here.
 */
val homeWidgets: List<@Composable () -> Unit> = listOf(
    { IntroText() },
    { StatusCard() },
    { PairingCard() },
    { PermissionsCard() },
)

/** What murmur actually is — shown until the phone is paired; after that
 *  the status card and service speak for themselves. */
@Composable
private fun IntroText() {
    val ctx = LocalContext.current
    val provisioned = remember { Secrets(ctx).provisioned }
    if (provisioned) return
    Text(
        "murmur quietly links this phone to your laptops: " +
            "notifications land on the desktop, the clipboard is shared, " +
            "and laptop media keys control playback here.",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 20.dp, vertical = 4.dp),
    )
}
