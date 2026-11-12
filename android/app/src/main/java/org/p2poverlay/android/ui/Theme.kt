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

/** Темы приложения: значение в настройках → подпись. Вампирские и фейские — с оформлением (ui/Decor.kt). */
val THEMES = listOf(
    "system" to "Системная", "light" to "Светлая", "dark" to "Тёмная",
    "blood" to "Кровавая луна", "night" to "Ночь в замке",
    "forest" to "Лесная фея", "bloom" to "Цветочная фея",
)

/** Палитра одной темы (ARGB). */
private class Pal(
    val rail: Long, val side: Long, val main: Long, val card: Long, val input: Long, val accent: Long, val accentDim: Long,
    val green: Long, val red: Long, val yellow: Long, val text: Long, val muted: Long, val bubbleIn: Long, val read: Long,
    val avatars: List<Long>,
)

private val BASE_AVATARS = listOf(0xFFEF6C6C, 0xFFF09A4A, 0xFFD9B33C, 0xFF52B788, 0xFF3FA7C9, 0xFF6C7BFF, 0xFFA86BE0, 0xFFE06BB4)

private val PALS = mapOf(
    "dark" to Pal(0xFF1A1B1F, 0xFF24262B, 0xFF2C2E34, 0xFF36383F, 0xFF1F2024, 0xFF6C7BFF, 0xFF4A55C9, 0xFF3BC47A, 0xFFF0565A,
        0xFFF2B83C, 0xFFE3E5E8, 0xFF9AA0AA, 0xFF36383F, 0xFF8FE3FF, BASE_AVATARS),
    "light" to Pal(0xFFF3F4F7, 0xFFFFFFFF, 0xFFE9EBF0, 0xFFFFFFFF, 0xFFE6E8ED, 0xFF4A55C9, 0xFF5C68E6, 0xFF1B9A55, 0xFFD3383D,
        0xFFB7791F, 0xFF1C1E23, 0xFF5E6571, 0xFFFFFFFF, 0xFFBFEFFF, BASE_AVATARS),
    "blood" to Pal(0xFF0E0405, 0xFF1C0809, 0xFF140607, 0xFF2B0E11, 0xFF0A0203, 0xFFD3162A, 0xFF8E0A17, 0xFF57B87A, 0xFFFF4040,
        0xFFE59A3A, 0xFFF3E4E4, 0xFFA8817F, 0xFF26090C, 0xFFFFB0A8,
        listOf(0xFFB3121F, 0xFF7A0E16, 0xFFD9483B, 0xFF8C5A3C, 0xFF9E2B4F, 0xFF5E1F2E, 0xFFC0392B, 0xFF6B4E4E)),
    "night" to Pal(0xFF0B0912, 0xFF16121F, 0xFF100D18, 0xFF221B2E, 0xFF0A0810, 0xFFB43A5E, 0xFF6E2240, 0xFF6FC7A0, 0xFFF0566E,
        0xFFE8D28C, 0xFFE9E5F0, 0xFF9C94AE, 0xFF201A2B, 0xFFE8D28C,
        listOf(0xFF6E2240, 0xFF4B3A78, 0xFF7A5BA6, 0xFF3D5A7A, 0xFF8C3B5E, 0xFF5A6B8C, 0xFFA0607F, 0xFF4E4566)),
    "forest" to Pal(0xFF0A1710, 0xFF10241A, 0xFF0D1D14, 0xFF1A3325, 0xFF07120C, 0xFF9BE564, 0xFF3E8E5A, 0xFF7BE0A0, 0xFFF07A7A,
        0xFFF4D35E, 0xFFE6F4E8, 0xFF93B39C, 0xFF18301F, 0xFFF4D35E,
        listOf(0xFF3E8E5A, 0xFF6B8E23, 0xFFB5651D, 0xFF2E7D6B, 0xFF8A9A3C, 0xFFC0504D, 0xFF5C7A3A, 0xFFA0784A)),
    "bloom" to Pal(0xFFF6ECF7, 0xFFFFF8FD, 0xFFFBF2F9, 0xFFFFFFFF, 0xFFF3E6F2, 0xFFB05FCB, 0xFFC77DD9, 0xFF4CAF7A, 0xFFE0567A,
        0xFFD9A21B, 0xFF3A2840, 0xFF8A7490, 0xFFFFFFFF, 0xFFFFE08A,
        listOf(0xFFE58FB1, 0xFFB07FD8, 0xFF7FB3E5, 0xFF8FD1A8, 0xFFF0B36B, 0xFFD98FD1, 0xFF9A8FE5, 0xFFE5A38F)),
)

/**
 * Палитра. Тёмная — как у desktop-клиента (desktop/src/theme.rs), светлая — та же гамма. Вампирские: «Кровавая луна»
 * (кровь на панелях и сообщениях) и «Ночь в замке» (луна, замок, летучие мыши); фейские: «Лесная фея» (лиана,
 * светлячки, мухоморы) и «Цветочная фея» (светлая: гирлянда цветов, пыльца, бабочки).
 * `dark` и `style` — состояние Compose: смена темы перерисовывает всё, что читает цвета.
 */
object P {
    var dark by mutableStateOf(true)
    /** default | blood | night | forest | bloom */
    var style by mutableStateOf("default")

    /** Тема с оформлением (вампирская или фейская). */
    val decorated: Boolean get() = style != "default"

    private val pal: Pal get() = PALS[if (style != "default") style else if (dark) "dark" else "light"]!!

    val Rail get() = Color(pal.rail)
    val Side get() = Color(pal.side)
    val Main get() = Color(pal.main)
    val Card get() = Color(pal.card)
    val Input get() = Color(pal.input)
    val Accent get() = Color(pal.accent)
    val AccentDim get() = Color(pal.accentDim)
    val Green get() = Color(pal.green)
    val Red get() = Color(pal.red)
    val Yellow get() = Color(pal.yellow)
    val Text get() = Color(pal.text)
    val Muted get() = Color(pal.muted)
    val Divider get() = Color(pal.card)
    /** Входящий пузырь сообщения. */
    val BubbleIn get() = Color(pal.bubbleIn)
    val Read get() = Color(pal.read)
    /** Фон экрана под списками: у тем с оформлением прозрачный — сквозь него видно оформление. */
    val Screen get() = if (decorated) Color.Transparent else Rail
    val ScreenMain get() = if (decorated) Color.Transparent else Main
    /** Кровь (капли, подтёки), лунный свет и серебро, светлячки, лепестки. */
    val Blood = Color(0xFFB0101E)
    val BloodDark = Color(0xFF5E040B)
    val Moon = Color(0xFFEDE6C8)
    val Silver = Color(0xFFB9B4C8)
    val Firefly = Color(0xFFF4E35E)
    val Gold = Color(0xFFE8B02E)
    val Petals = listOf(Color(0xFFF29BC4), Color(0xFFC99BF2), Color(0xFFFFC87A))
    val Avatars get() = pal.avatars.map { Color(it) }
}

@Composable
fun P2PTheme(theme: String = "system", content: @Composable () -> Unit) {
    val sysDark = isSystemInDarkTheme()
    P.style = if (theme in listOf("blood", "night", "forest", "bloom")) theme else "default"
    P.dark = when (theme) { "light", "bloom" -> false; "dark", "blood", "night", "forest" -> true; else -> sysDark }
    val base = Typography()
    // у тем с оформлением заголовки — антиквой
    val head = if (P.decorated) FontFamily.Serif else FontFamily.Default
    val scheme = if (P.dark) darkColorScheme(
        primary = P.Accent, onPrimary = Color.White, primaryContainer = P.AccentDim, onPrimaryContainer = Color.White,
        secondary = P.Green, background = P.Rail, onBackground = P.Text, surface = P.Side, onSurface = P.Text,
        surfaceVariant = P.Card, onSurfaceVariant = P.Muted, surfaceContainer = P.Side, surfaceContainerHigh = P.Card,
        surfaceContainerHighest = P.Card, surfaceContainerLow = P.Main,
        outline = P.Muted.copy(alpha = 0.45f),
        outlineVariant = P.Muted.copy(alpha = 0.25f),
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
