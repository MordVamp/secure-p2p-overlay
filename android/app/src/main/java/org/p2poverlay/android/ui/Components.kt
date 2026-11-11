package org.p2poverlay.android.ui

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.material.icons.filled.Lock
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

fun colorFor(id: String): Color {
    var h = 5381
    id.forEach { h = (h * 33) xor it.code }
    return P.Avatars[Math.floorMod(h, P.Avatars.size)]
}

@Composable
fun Avatar(key: String, label: String, size: Dp = 44.dp) {
    val initials = label.trimStart('@').split(' ').filter { it.isNotBlank() }.take(2)
        .mapNotNull { w -> w.firstOrNull { it.isLetterOrDigit() } }.joinToString("").uppercase()
    Box(
        Modifier.size(size).clip(CircleShape).background(colorFor(key)),
        contentAlignment = Alignment.Center,
    ) {
        Text(initials, color = Color.White, fontWeight = FontWeight.SemiBold, fontSize = (size.value * 0.38f).sp)
    }
}

@Composable
fun Pill(text: String, color: Color) {
    Box(
        Modifier.clip(RoundedCornerShape(10.dp)).background(color.copy(alpha = 0.16f))
            .border(1.dp, color.copy(alpha = 0.55f), RoundedCornerShape(10.dp))
            .padding(horizontal = 8.dp, vertical = 2.dp)
    ) { Text(text, color = color, fontSize = 12.sp) }
}

@Composable
fun SectionCard(title: String? = null, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Surface(modifier.fillMaxWidth(), shape = RoundedCornerShape(16.dp), color = P.Card,
        shadowElevation = if (P.dark) 0.dp else 1.dp) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (title != null) Text(title.uppercase(), style = MaterialTheme.typography.labelSmall, color = P.Muted)
            content()
        }
    }
}

@Composable
fun KeyValue(k: String, v: String, mono: Boolean = false) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(k, color = P.Muted, fontSize = 14.sp, modifier = Modifier.weight(0.38f))
        Text(v, fontSize = if (mono) 12.sp else 14.sp, fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default,
            modifier = Modifier.weight(0.62f))
    }
}

@Composable
fun QrImage(text: String, size: Dp = 220.dp) {
    val bmp = remember(text) {
        val m = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 0, 0, mapOf(EncodeHintType.MARGIN to 2))
        val b = Bitmap.createBitmap(m.width, m.height, Bitmap.Config.ARGB_8888)
        for (y in 0 until m.height) for (x in 0 until m.width)
            b.setPixel(x, y, if (m.get(x, y)) 0xFF000000.toInt() else 0xFFFFFFFF.toInt())
        b.asImageBitmap()
    }
    Image(bmp, "QR", Modifier.size(size).clip(RoundedCornerShape(12.dp)), filterQuality = FilterQuality.None)
}

fun hhmm(ts: Long): String = SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date(ts))

fun whenShort(ts: Long): String {
    val now = System.currentTimeMillis()
    return if (now - ts < 20 * 3600_000L) hhmm(ts) else SimpleDateFormat("d MMM", Locale.forLanguageTag("ru")).format(Date(ts))
}

fun humanBytes(n: Long): String = when {
    n < 1024 -> "$n Б"
    n < 1024 * 1024 -> "%.1f КиБ".format(n / 1024.0)
    else -> "%.1f МиБ".format(n / 1048576.0)
}

private val imageExt = setOf("jpg", "jpeg", "png", "webp", "gif", "bmp", "heic")

fun isImage(name: String) = name.substringAfterLast('.', "").lowercase() in imageExt

/** Уменьшенная картинка из файла (декодируется с прореживанием, не целиком). */
@Composable
fun Thumbnail(file: java.io.File, maxSide: Dp = 220.dp, onClick: () -> Unit = {}) {
    val px = with(androidx.compose.ui.platform.LocalDensity.current) { maxSide.roundToPx() }
    val bmp = remember(file.path, file.length()) {
        runCatching {
            val o = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
            android.graphics.BitmapFactory.decodeFile(file.path, o)
            var s = 1
            while (o.outWidth / (s * 2) >= px && o.outHeight / (s * 2) >= px) s *= 2
            android.graphics.BitmapFactory.decodeFile(file.path,
                android.graphics.BitmapFactory.Options().apply { inSampleSize = s })?.asImageBitmap()
        }.getOrNull()
    }
    if (bmp == null) return
    val ratio = bmp.width.toFloat() / bmp.height.coerceAtLeast(1)
    Image(
        bmp, file.name,
        Modifier.size(if (ratio >= 1) maxSide else maxSide * ratio, if (ratio >= 1) maxSide / ratio else maxSide)
            .clip(RoundedCornerShape(12.dp))
            .then(Modifier.clickableNoRipple(onClick)),
        contentScale = androidx.compose.ui.layout.ContentScale.Crop,
    )
}

private fun Modifier.clickableNoRipple(onClick: () -> Unit): Modifier = this.clickable(onClick = onClick)

/** Номер безопасности: 12 групп по 5 цифр в сетке 4×3, крупным моноширинным шрифтом. */
@Composable
fun SafetyGrid(safety: String) {
    val g = safety.split(' ')
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        g.chunked(4).forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                row.forEach { Text(it, fontFamily = FontFamily.Monospace, fontSize = 18.sp, fontWeight = FontWeight.Medium) }
            }
        }
    }
}


/** Экран блокировки (пока не пройдена биометрия / PIN). */
@Composable
fun LockScreen(onUnlock: () -> Unit) {
    Column(
        Modifier.fillMaxSize().background(P.Rail).padding(32.dp),
        verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.Filled.Lock, null, tint = P.Accent, modifier = Modifier.size(64.dp))
        androidx.compose.foundation.layout.Spacer(Modifier.size(16.dp))
        Text("Приложение заблокировано", fontSize = 20.sp, color = P.Text)
        androidx.compose.foundation.layout.Spacer(Modifier.size(16.dp))
        androidx.compose.material3.Button(onClick = onUnlock) { Text("Разблокировать") }
    }
}
