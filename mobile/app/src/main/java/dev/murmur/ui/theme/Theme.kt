package dev.murmur.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// Gruvbox, mapped onto Material 3 roles. murmur has its own palette on
// purpose — no dynamic Material You; the look is the product's.

private object Gb {
    // dark
    val bg0h = Color(0xFF1D2021)
    val bg0 = Color(0xFF282828)
    val bg1 = Color(0xFF3C3836)
    val bg2 = Color(0xFF504945)
    val bg3 = Color(0xFF665C54)
    val fg0 = Color(0xFFFBF1C7)
    val fg1 = Color(0xFFEBDBB2)
    val fg4 = Color(0xFFA89984)
    val gray = Color(0xFF928374)
    val red = Color(0xFFFB4934)
    val green = Color(0xFFB8BB26)
    val yellow = Color(0xFFFABD2F)
    val blue = Color(0xFF83A598)
    val purple = Color(0xFFD3869B)
    val aqua = Color(0xFF8EC07C)
    val orange = Color(0xFFFE8019)
    // light
    val lBg = Color(0xFFFBF1C7)
    val lCard = Color(0xFFEBDBB2)
    val lCardHigh = Color(0xFFD5C4A1)
    val lFg = Color(0xFF3C3836)
    val lFg2 = Color(0xFF665C54)
    val lGray = Color(0xFF928374)
    val lRed = Color(0xFF9D0006)
    val lGreen = Color(0xFF79740E)
    val lYellow = Color(0xFFB57614)
    val lAqua = Color(0xFF427B58)
    val lPurple = Color(0xFF8F3F71)
    val lOrange = Color(0xFFAF3A03)
}

private val DarkScheme = darkColorScheme(
    primary = Gb.yellow,
    onPrimary = Gb.bg0,
    primaryContainer = Gb.bg2,
    onPrimaryContainer = Gb.yellow,
    secondary = Gb.aqua,
    onSecondary = Gb.bg0,
    secondaryContainer = Gb.bg2,
    onSecondaryContainer = Gb.aqua,
    tertiary = Gb.purple,
    onTertiary = Gb.bg0,
    tertiaryContainer = Gb.bg2,
    onTertiaryContainer = Gb.purple,
    error = Gb.red,
    onError = Gb.bg0,
    errorContainer = Gb.bg2,
    onErrorContainer = Gb.red,
    background = Gb.bg0h,
    onBackground = Gb.fg1,
    surface = Gb.bg0,
    onSurface = Gb.fg1,
    surfaceVariant = Gb.bg2,
    onSurfaceVariant = Gb.fg4,
    surfaceContainerLowest = Gb.bg0h,
    surfaceContainerLow = Gb.bg0,
    surfaceContainer = Gb.bg1,
    surfaceContainerHigh = Gb.bg2,
    surfaceContainerHighest = Gb.bg3,
    outline = Gb.bg3,
    outlineVariant = Gb.bg2,
)

private val LightScheme = lightColorScheme(
    primary = Gb.lOrange,
    onPrimary = Gb.lBg,
    primaryContainer = Gb.lCardHigh,
    onPrimaryContainer = Gb.lOrange,
    secondary = Gb.lAqua,
    onSecondary = Gb.lBg,
    secondaryContainer = Gb.lCardHigh,
    onSecondaryContainer = Gb.lAqua,
    tertiary = Gb.lPurple,
    onTertiary = Gb.lBg,
    tertiaryContainer = Gb.lCardHigh,
    onTertiaryContainer = Gb.lPurple,
    error = Gb.lRed,
    onError = Gb.lBg,
    errorContainer = Gb.lCardHigh,
    onErrorContainer = Gb.lRed,
    background = Gb.lBg,
    onBackground = Gb.lFg,
    surface = Gb.lCard,
    onSurface = Gb.lFg,
    surfaceVariant = Gb.lCardHigh,
    onSurfaceVariant = Gb.lFg2,
    surfaceContainerLowest = Gb.lBg,
    surfaceContainerLow = Gb.lCard,
    surfaceContainer = Gb.lCard,
    surfaceContainerHigh = Gb.lCardHigh,
    surfaceContainerHighest = Gb.lCardHigh,
    outline = Gb.lGray,
    outlineVariant = Gb.lCardHigh,
)

@Composable
fun MurmurTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (isSystemInDarkTheme()) DarkScheme else LightScheme,
        typography = MurmurTypography,
        content = content,
    )
}
