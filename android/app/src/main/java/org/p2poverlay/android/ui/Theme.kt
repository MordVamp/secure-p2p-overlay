package org.p2poverlay.android.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/**
 * Палитра. Тёмная — как у desktop-клиента (desktop/src/theme.rs), светлая — та же гамма.
 * `dark` — состояние Compose: смена темы перерисовывает всё, что читает цвета.
 */
object P {
    var dark by mutableStateOf(true)

    private fun c(d: Long, l: Long) = Color(if (dark) d else l)

    val Rail get() = c(0xFF1A1B1F, 0xFFF3F4F7)
    val Side get() = c(0xFF24262B, 0xFFFFFFFF)
    val Main get() = c(0xFF2C2E34, 0xFFE9EBF0)
    val Card get() = c(0xFF36383F, 0xFFFFFFFF)
    val Input get() = c(0xFF1F2024, 0xFFE6E8ED)
    val Accent get() = c(0xFF6C7BFF, 0xFF4A55C9)
    val AccentDim get() = c(0xFF4A55C9, 0xFF5C68E6)
    val Green get() = c(0xFF3BC47A, 0xFF1B9A55)
    val Red get() = c(0xFFF0565A, 0xFFD3383D)
    val Yellow get() = c(0xFFF2B83C, 0xFFB7791F)
    val Text get() = c(0xFFE3E5E8, 0xFF1C1E23)
    val Muted get() = c(0xFF9AA0AA, 0xFF5E6571)
    val Divider get() = c(0xFF24262B, 0xFFE3E5EA)
    /** Входящий пузырь сообщения. */
    val BubbleIn get() = c(0xFF36383F, 0xFFFFFFFF)
    val Read get() = c(0xFF8FE3FF, 0xFFBFEFFF)
    val Avatars = listOf(
        Color(0xFFEF6C6C), Color(0xFFF09A4A), Color(0xFFD9B33C), Color(0xFF52B788),
        Color(0xFF3FA7C9), Color(0xFF6C7BFF), Color(0xFFA86BE0), Color(0xFFE06BB4),
    )
}

@Composable
fun P2PTheme(theme: String = "system", content: @Composable () -> Unit) {
    P.dark = when (theme) { "light" -> false; "dark" -> true; else -> isSystemInDarkTheme() }
    val base = Typography()
    val scheme = if (P.dark) darkColorScheme(
        primary = P.Accent, onPrimary = Color.White, primaryContainer = P.AccentDim, onPrimaryContainer = Color.White,
        secondary = P.Green, background = P.Rail, onBackground = P.Text, surface = P.Side, onSurface = P.Text,
        surfaceVariant = P.Card, onSurfaceVariant = P.Muted, surfaceContainer = P.Side, surfaceContainerHigh = P.Card,
        surfaceContainerHighest = P.Card, surfaceContainerLow = P.Main, outline = Color(0xFF4A4D55),
        outlineVariant = Color(0xFF3A3C43), error = P.Red,
    ) else lightColorScheme(
        primary = P.Accent, onPrimary = Color.White, primaryContainer = P.AccentDim, onPrimaryContainer = Color.White,
        secondary = P.Green, background = P.Rail, onBackground = P.Text, surface = P.Side, onSurface = P.Text,
        surfaceVariant = P.Card, onSurfaceVariant = P.Muted, surfaceContainer = P.Side, surfaceContainerHigh = P.Card,
        surfaceContainerHighest = Color(0xFFF0F1F5), surfaceContainerLow = P.Main, outline = Color(0xFFB9BDC6),
        outlineVariant = Color(0xFFDADDE3), error = P.Red,
    )
    MaterialTheme(
        colorScheme = scheme,
        typography = base.copy(
            titleLarge = base.titleLarge.copy(fontWeight = FontWeight.SemiBold),
            labelSmall = TextStyle(fontSize = 11.sp, letterSpacing = 0.6.sp, fontWeight = FontWeight.SemiBold),
        ),
        content = content,
    )
}
