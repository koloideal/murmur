package dev.murmur.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

// murmur accent: quiet indigo-violet, same family as the launcher icon.
private val MurmurSeed = Color(0xFF7C6FD8)

private val LightScheme = lightColorScheme(
    primary = MurmurSeed,
    secondary = Color(0xFF625B8F),
    tertiary = Color(0xFF7D5260),
)

private val DarkScheme = darkColorScheme(
    primary = Color(0xFFC9BFFF),
    secondary = Color(0xFFCCC2DC),
    tertiary = Color(0xFFEFB8C8),
)

@Composable
fun MurmurTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val scheme = when {
        // Material You — the wallpaper-tinted scheme is the native look.
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val ctx = LocalContext.current
            if (dark) dynamicDarkColorScheme(ctx) else dynamicLightColorScheme(ctx)
        }
        dark -> DarkScheme
        else -> LightScheme
    }
    MaterialTheme(colorScheme = scheme, content = content)
}
