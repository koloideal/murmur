package dev.murmur.ui.widgets

import androidx.compose.runtime.Composable

/**
 * Home-screen widget registry. A feature's UI is a self-contained card
 * composable that owns its state — add a feature to the home screen by
 * writing the card file and appending one line here.
 */
val homeWidgets: List<@Composable () -> Unit> = listOf(
    { StatusCard() },
    { DevicesCard() },
    { NotificationsCard() },
    { ClipboardCard() },
)
