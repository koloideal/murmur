@file:OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)

package dev.murmur.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import dev.murmur.R

// Fonts borrowed from tunnex.dev: Unbounded for display, Onest for text —
// both OFL, variable, with proper Cyrillic coverage. Bundled as single
// wght-axis TTFs; FontVariation picks the instance per weight.

private fun onest(wght: Int) = Font(
    R.font.onest,
    FontWeight(wght),
    variationSettings = FontVariation.Settings(FontVariation.weight(wght)),
)

private fun unbounded(wght: Int) = Font(
    R.font.unbounded,
    FontWeight(wght),
    variationSettings = FontVariation.Settings(FontVariation.weight(wght)),
)

val Onest = FontFamily(onest(400), onest(500), onest(600), onest(700))
val Unbounded = FontFamily(unbounded(500), unbounded(600), unbounded(700), unbounded(800))

val MurmurTypography = Typography().run {
    copy(
        displayLarge = displayLarge.copy(fontFamily = Unbounded),
        displayMedium = displayMedium.copy(fontFamily = Unbounded),
        displaySmall = displaySmall.copy(fontFamily = Unbounded),
        headlineLarge = headlineLarge.copy(fontFamily = Unbounded, fontWeight = FontWeight.SemiBold),
        headlineMedium = headlineMedium.copy(fontFamily = Unbounded, fontWeight = FontWeight.SemiBold),
        headlineSmall = headlineSmall.copy(fontFamily = Unbounded, fontWeight = FontWeight.SemiBold),
        titleLarge = titleLarge.copy(fontFamily = Unbounded, fontWeight = FontWeight.SemiBold),
        titleMedium = titleMedium.copy(fontFamily = Onest, fontWeight = FontWeight.SemiBold),
        bodyLarge = bodyLarge.copy(fontFamily = Onest),
        bodyMedium = bodyMedium.copy(fontFamily = Onest),
        bodySmall = bodySmall.copy(fontFamily = Onest),
        labelLarge = labelLarge.copy(fontFamily = Onest, fontWeight = FontWeight.SemiBold),
        labelMedium = labelMedium.copy(fontFamily = Onest),
        labelSmall = labelSmall.copy(fontFamily = Onest),
    )
}
