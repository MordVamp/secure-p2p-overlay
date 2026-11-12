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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/** Темы приложения: значение в настройках → подпись. Вампирские — со своим оформлением (ui/Decor.kt). */
val THEMES = listOf(
    "system" to "Системная", "light" to "Светлая", "dark" to "Тёмная",
    "blood" to "Кровавая луна", "night" to "Ночь в замке",
)

/**
 * Палитра. Тёмная — как у desktop-клиента (desktop/src/theme.rs), светлая — та же гамма.
 * Вампирские: «Кровавая луна» — чёрно-багровая, кровь на панелях и сообщениях; «Ночь в замке» — фиолетово-чёрная
 * с винным акцентом и серебром, луна, замок и летучие мыши (без крови).
 * `dark` и `style` — состояние Compose: смена темы перерисовывает всё, что читает цвета.
 */
object P {
    var dark by mutableStateOf(true)
    /** default | blood | night */
    var style by mutableStateOf("default")

    val vampire: Boolean get() = style != "default"

    private fun c(d: Long, l: Long, blood: Long = d, night: Long = d) =
        Color(when (style) { "blood" -> blood; "night" -> night; else -> if (dark) d else l })

    val Rail get() = c(0xFF1A1B1F, 0xFFF3F4F7, 0xFF0E0405, 0xFF0B0912)
    val Side get() = c(0xFF24262B, 0xFFFFFFFF, 0xFF1C0809, 0xFF16121F)
    val Main get() = c(0xFF2C2E34, 0xFFE9EBF0, 0xFF140607, 0xFF100D18)
    val Card get() = c(0xFF36383F, 0xFFFFFFFF, 0xFF2B0E11, 0xFF221B2E)
    val Input get() = c(0xFF1F2024, 0xFFE6E8ED, 0xFF0A0203, 0xFF0A0810)
    val Accent get() = c(0xFF6C7BFF, 0xFF4A55C9, 0xFFD3162A, 0xFFB43A5E)
    val AccentDim get() = c(0xFF4A55C9, 0xFF5C68E6, 0xFF8E0A17, 0xFF6E2240)
    val Green get() = c(0xFF3BC47A, 0xFF1B9A55, 0xFF57B87A, 0xFF6FC7A0)
    val Red get() = c(0xFFF0565A, 0xFFD3383D, 0xFFFF4040, 0xFFF0566E)
    val Yellow get() = c(0xFFF2B83C, 0xFFB7791F, 0xFFE59A3A, 0xFFE8D28C)
    val Text get() = c(0xFFE3E5E8, 0xFF1C1E23, 0xFFF3E4E4, 0xFFE9E5F0)
    val Muted get() = c(0xFF9AA0AA, 0xFF5E6571, 0xFFA8817F, 0xFF9C94AE)
    val Divider get() = c(0xFF24262B, 0xFFE3E5EA, 0xFF2B0E11, 0xFF221B2E)
    /** Входящий пузырь сообщения. */
    val BubbleIn get() = c(0xFF36383F, 0xFFFFFFFF, 0xFF26090C, 0xFF201A2B)
    val Read get() = c(0xFF8FE3FF, 0xFFBFEFFF, 0xFFFFB0A8, 0xFFE8D28C)
    /** Фон экрана под списками: у вампирских тем прозрачный — сквозь него видно оформление. */
    val Screen get() = if (vampire) Color.Transparent else Rail
    val ScreenMain get() = if (vampire) Color.Transparent else Main
    /** Кровь (капли, подтёки) и серебро/лунный свет. */
    val Blood = Color(0xFFB0101E)
    val BloodDark = Color(0xFF5E040B)
    val Moon = Color(0xFFEDE6C8)
    val Silver = Color(0xFFB9B4C8)
    val Avatars get() = when (style) {
        "blood" -> listOf(Color(0xFFB3121F), Color(0xFF7A0E16), Color(0xFFD9483B), Color(0xFF8C5A3C),
            Color(0xFF9E2B4F), Color(0xFF5E1F2E), Color(0xFFC0392B), Color(0xFF6B4E4E))
        "night" -> listOf(Color(0xFF6E2240), Color(0xFF4B3A78), Color(0xFF7A5BA6), Color(0xFF3D5A7A),
            Color(0xFF8C3B5E), Color(0xFF5A6B8C), Color(0xFFA0607F), Color(0xFF4E4566))
        else -> listOf(
            Color(0xFFEF6C6C), Color(0xFFF09A4A), Color(0xFFD9B33C), Color(0xFF52B788),
            Color(0xFF3FA7C9), Color(0xFF6C7BFF), Color(0xFFA86BE0), Color(0xFFE06BB4),
        )
    }
}

@Composable
fun P2PTheme(theme: String = "system", content: @Composable () -> Unit) {
    val sysDark = isSystemInDarkTheme()
    P.style = if (theme == "blood" || theme == "night") theme else "default"
    P.dark = when (theme) { "light" -> false; "dark", "blood", "night" -> true; else -> sysDark }
    val base = Typography()
    // у вампирских тем заголовки — антиквой (готичнее, чем гротеск)
    val head = if (P.vampire) FontFamily.Serif else FontFamily.Default
    val scheme = if (P.dark) darkColorScheme(
        primary = P.Accent, onPrimary = Color.White, primaryContainer = P.AccentDim, onPrimaryContainer = Color.White,
        secondary = P.Green, background = P.Rail, onBackground = P.Text, surface = P.Side, onSurface = P.Text,
        surfaceVariant = P.Card, onSurfaceVariant = P.Muted, surfaceContainer = P.Side, surfaceContainerHigh = P.Card,
        surfaceContainerHighest = P.Card, surfaceContainerLow = P.Main,
        outline = when (P.style) { "blood" -> Color(0xFF5A1A1F); "night" -> Color(0xFF4A3F5C); else -> Color(0xFF4A4D55) },
        outlineVariant = when (P.style) { "blood" -> Color(0xFF3A1014); "night" -> Color(0xFF2E263B); else -> Color(0xFF3A3C43) },
        error = P.Red,
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
            titleLarge = base.titleLarge.copy(fontWeight = FontWeight.SemiBold, fontFamily = head),
            headlineSmall = base.headlineSmall.copy(fontFamily = head),
            labelSmall = TextStyle(fontSize = 11.sp, letterSpacing = 0.6.sp, fontWeight = FontWeight.SemiBold),
        ),
        content = content,
    )
}
