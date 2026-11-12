package org.p2poverlay.android.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Выбор темы: карточки с миниатюрой (у вампирских — капля крови / луна с летучей мышью). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ThemePicker(current: String, onPick: (String) -> Unit) {
    FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        THEMES.forEach { (v, label) ->
            val sel = current == v
            Column(
                Modifier.width(96.dp).clip(RoundedCornerShape(12.dp))
                    .border(if (sel) 2.dp else 1.dp, if (sel) P.Accent else P.Muted.copy(alpha = 0.3f), RoundedCornerShape(12.dp))
                    .clickable { onPick(v) }.padding(6.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Canvas(Modifier.fillMaxWidth().height(54.dp).clip(RoundedCornerShape(8.dp))) { preview(v) }
                Text(label, fontSize = 12.sp, color = if (sel) P.Text else P.Muted, textAlign = TextAlign.Center,
                    modifier = Modifier.padding(top = 4.dp))
            }
        }
    }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.preview(v: String) {
    val (bg, card, accent) = when (v) {
        "light" -> Triple(Color(0xFFF3F4F7), Color.White, Color(0xFF4A55C9))
        "blood" -> Triple(Color(0xFF0E0405), Color(0xFF2B0E11), Color(0xFF8E0A17))
        "night" -> Triple(Color(0xFF0B0912), Color(0xFF221B2E), Color(0xFF6E2240))
        "forest" -> Triple(Color(0xFF0D1D14), Color(0xFF1A3325), Color(0xFF3E8E5A))
        "bloom" -> Triple(Color(0xFFFBF2F9), Color.White, Color(0xFFC77DD9))
        "system" -> Triple(Color(0xFF1A1B1F), Color(0xFFF3F4F7), Color(0xFF6C7BFF))
        else -> Triple(Color(0xFF1A1B1F), Color(0xFF36383F), Color(0xFF4A55C9))
    }
    drawRect(bg)
    if (v == "system") drawRect(Color(0xFFF3F4F7), Offset(size.width / 2, 0f), Size(size.width / 2, size.height))
    val u = size.width / 10
    drawRoundRect(card, Offset(u, size.height * 0.45f), Size(u * 5, size.height * 0.2f), androidx.compose.ui.geometry.CornerRadius(u))
    drawRoundRect(accent, Offset(u * 4, size.height * 0.72f), Size(u * 5, size.height * 0.2f), androidx.compose.ui.geometry.CornerRadius(u))
    when (v) {
        "blood" -> {
            drawRect(Color(0xFF5E040B), size = Size(size.width, size.height * 0.12f))
            for ((x, l) in listOf(0.15f to 0.22f, 0.42f to 0.34f, 0.7f to 0.18f, 0.88f to 0.28f)) {
                drawRect(Color(0xFFB0101E), Offset(size.width * x - u * 0.3f, 0f), Size(u * 0.6f, size.height * l))
                drawCircle(Color(0xFFB0101E), u * 0.42f, Offset(size.width * x, size.height * l))
            }
        }
        "forest" -> {
            for ((x, y) in listOf(0.55f to 0.3f, 0.7f to 0.2f, 0.85f to 0.35f)) {
                drawCircle(Color(0xFFF4E35E).copy(alpha = 0.25f), u * 0.6f, Offset(size.width * x, size.height * y))
                drawCircle(Color(0xFFF4E35E), u * 0.22f, Offset(size.width * x, size.height * y))
            }
            drawArc(Color(0xFFC82A22), 180f, 180f, true, Offset(u * 0.6f, size.height * 0.55f), Size(u * 2.2f, u * 1.8f))
            drawRect(Color(0xFFF1E6D2), Offset(u * 1.45f, size.height * 0.55f + u * 0.9f), Size(u * 0.5f, u * 1.4f))
        }
        "bloom" -> {
            for (k in 0 until 5) {
                val a = k / 5f * 6.2832f
                drawCircle(Color(0xFFF29BC4), u * 0.45f, Offset(u * 1.6f + kotlin.math.cos(a) * u * 0.55f, size.height * 0.3f + kotlin.math.sin(a) * u * 0.55f))
            }
            drawCircle(Color(0xFFFFE08A), u * 0.35f, Offset(u * 1.6f, size.height * 0.3f))
            drawCircle(Color(0xFFE8B02E), u * 0.18f, Offset(size.width * 0.8f, size.height * 0.2f))
        }
        "night" -> {
            drawCircle(Color(0xFFEDE6C8), u * 1.3f, Offset(size.width * 0.78f, size.height * 0.24f))
            drawCircle(Color(0xFF0B0912), u * 0.5f, Offset(size.width * 0.3f, size.height * 0.2f))
            drawCircle(Color(0xFF0B0912), u * 0.5f, Offset(size.width * 0.42f, size.height * 0.2f))
            drawRect(Color(0xFF07050C), Offset(size.width * 0.36f - u * 0.2f, size.height * 0.15f), Size(u * 0.4f, u * 0.6f))
        }
    }
}
