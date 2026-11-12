package org.p2poverlay.android.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.sin

/**
 * Оформление вампирских тем — рисуется на Canvas, без картинок:
 *  • «Кровавая луна»: кровь стекает с нижнего края панелей (капли срываются и падают), брызги на фоне,
 *    подтёки под исходящими сообщениями, кровавая луна на экране звонка;
 *  • «Ночь в замке»: готические арки под панелями, звёзды, луна, силуэт замка с окнами и летучие мыши.
 * При обычных темах все функции ничего не рисуют.
 */
object Decor {
    /** Детерминированный «случайный» ряд: оформление не прыгает при перерисовке. */
    private fun rnd(i: Int, salt: Int = 0): Float {
        val x = sin((i * 12.9898 + salt * 78.233)) * 43758.5453
        return (x - kotlin.math.floor(x)).toFloat()
    }

    /** Значок у названия приложения. */
    val glyph: String get() = when (P.style) { "blood" -> "🩸 "; "night" -> "🦇 "; else -> "" }

    /** Нижний край верхней панели: подтёки крови / готические арки. */
    @Composable
    fun TopEdge(modifier: Modifier = Modifier) {
        when (P.style) {
            "blood" -> Drips(modifier.fillMaxWidth().height(34.dp), P.Side)
            "night" -> Arches(modifier.fillMaxWidth().height(12.dp), P.Side)
        }
    }

    @Composable
    private fun Drips(modifier: Modifier, band: Color) {
        val t = rememberInfiniteTransition(label = "drip")
        val fall by t.animateFloat(0f, 1f, infiniteRepeatable(tween(3600, easing = LinearEasing)), label = "fall")
        val fall2 by t.animateFloat(0f, 1f, infiniteRepeatable(tween(5300, delayMillis = 900, easing = LinearEasing)), label = "fall2")
        Canvas(modifier) {
            val top = 4.dp.toPx()
            drawRect(band, size = Size(size.width, top))
            // неровная кромка, с которой стекает кровь
            val edge = Path().apply {
                moveTo(0f, 0f)
                var x = 0f
                var i = 0
                while (x < size.width) {
                    lineTo(x, top + rnd(i, 1) * 3.dp.toPx())
                    x += 10.dp.toPx()
                    i++
                }
                lineTo(size.width, top); lineTo(size.width, 0f); close()
            }
            drawPath(edge, Brush.verticalGradient(listOf(P.BloodDark, P.Blood), 0f, top + 3.dp.toPx()))
            var x = 8.dp.toPx()
            var i = 0
            var longest = Offset.Zero
            var second = Offset.Zero
            while (x < size.width) {
                val w = (4.5f + rnd(i, 2) * 5f).dp.toPx()
                val len = (4f + rnd(i, 3) * rnd(i, 5) * 22f).dp.toPx()
                drip(Offset(x, top), w, len)
                if (len > longest.y) { second = longest; longest = Offset(x, len) } else if (len > second.y) second = Offset(x, len)
                x += (16f + rnd(i, 4) * 38f).dp.toPx()
                i++
            }
            // капля срывается с самого длинного подтёка и падает
            fallingDrop(Offset(longest.x, top + longest.y), fall)
            fallingDrop(Offset(second.x, top + second.y), fall2)
        }
    }

    private fun DrawScope.drip(at: Offset, w: Float, len: Float) {
        val r = w / 2f
        val p = Path().apply {
            moveTo(at.x - r * 0.9f, at.y - 1f)
            cubicTo(at.x - r * 0.5f, at.y + len * 0.4f, at.x - r, at.y + len - r, at.x - r, at.y + len - r)
            arcTo(androidx.compose.ui.geometry.Rect(at.x - r, at.y + len - 2 * r, at.x + r, at.y + len), 180f, -180f, false)
            cubicTo(at.x + r, at.y + len - r, at.x + r * 0.5f, at.y + len * 0.4f, at.x + r * 0.9f, at.y - 1f)
            close()
        }
        drawPath(p, Brush.verticalGradient(listOf(P.BloodDark, P.Blood), at.y, at.y + len))
        // блик — кровь выглядит влажной
        drawCircle(Color.White.copy(alpha = 0.28f), r * 0.3f, Offset(at.x - r * 0.35f, at.y + len - r * 1.2f))
    }

    private fun DrawScope.fallingDrop(from: Offset, p: Float) {
        if (from == Offset.Zero) return
        // 0..0.55 — набухает на кончике, затем падает и тает
        val r = 2.2.dp.toPx()
        if (p < 0.55f) {
            drawCircle(P.Blood, r * (0.4f + p), Offset(from.x, from.y))
        } else {
            val q = (p - 0.55f) / 0.45f
            val y = from.y + q * q * (size.height - from.y + 6.dp.toPx())
            drawCircle(P.Blood.copy(alpha = 1f - q * 0.7f), r, Offset(from.x, y))
        }
    }

    @Composable
    private fun Arches(modifier: Modifier, band: Color) {
        Canvas(modifier) {
            val w = 18.dp.toPx()
            val h = size.height
            val p = Path().apply {
                moveTo(0f, 0f)
                var x = 0f
                while (x < size.width) {
                    // стрельчатая арка: две дуги сходятся остриём вниз
                    quadraticTo(x + w * 0.1f, h * 0.9f, x + w / 2, h)
                    quadraticTo(x + w * 0.9f, h * 0.9f, x + w, 0f)
                    x += w
                }
                lineTo(size.width, 0f); close()
            }
            drawPath(p, band)
            drawPath(p, P.Silver.copy(alpha = 0.35f), style = Stroke(1f))
        }
    }

    /** Фон под списками. */
    @Composable
    fun Backdrop(modifier: Modifier = Modifier) {
        when (P.style) {
            "blood" -> Canvas(modifier) { splatter(0.10f) }
            "night" -> NightSky(modifier, strong = false)
        }
    }

    /** Фон экрана звонка — ярче, чем под списками. */
    @Composable
    fun CallBackdrop(modifier: Modifier = Modifier) {
        when (P.style) {
            "blood" -> {
                val t = rememberInfiniteTransition(label = "moon")
                val glow by t.animateFloat(0.55f, 0.8f, infiniteRepeatable(tween(2600), RepeatMode.Reverse), label = "g")
                Canvas(modifier) {
                    val c = Offset(size.width / 2, size.height * 0.26f)
                    val r = size.width * 0.36f
                    drawCircle(Brush.radialGradient(listOf(P.Blood.copy(alpha = glow * 0.55f), Color.Transparent), c, r * 1.9f), r * 1.9f, c)
                    drawCircle(Brush.radialGradient(listOf(Color(0xFFE0353F), Color(0xFF7E0710)), c + Offset(-r * 0.3f, -r * 0.3f), r * 1.4f), r, c)
                    for (i in 0 until 5) drawCircle(Color(0xFF5E040B).copy(alpha = 0.35f), r * (0.08f + rnd(i, 9) * 0.1f),
                        c + Offset((rnd(i, 10) - 0.5f) * r * 1.2f, (rnd(i, 11) - 0.5f) * r * 1.2f))
                    splatter(0.16f)
                }
                Drips(Modifier.fillMaxWidth().height(40.dp), P.BloodDark)
            }
            "night" -> NightSky(modifier, strong = true)
        }
    }

    private fun DrawScope.splatter(alpha: Float) {
        for (k in 0 until 7) {
            val cx = rnd(k, 20) * size.width
            val cy = (0.15f + rnd(k, 21) * 0.85f) * size.height
            val base = (6f + rnd(k, 22) * 16f).dp.toPx()
            drawCircle(P.Blood.copy(alpha = alpha), base, Offset(cx, cy))
            for (j in 0 until 9) {
                val a = rnd(k * 31 + j, 23) * 2 * PI
                val d = base * (1.2f + rnd(k * 31 + j, 24) * 2.2f)
                drawCircle(P.Blood.copy(alpha = alpha * 0.9f), base * (0.08f + rnd(k * 31 + j, 25) * 0.22f),
                    Offset(cx + (kotlin.math.cos(a) * d).toFloat(), cy + (sin(a) * d).toFloat()))
            }
        }
    }

    @Composable
    private fun NightSky(modifier: Modifier, strong: Boolean) {
        val t = rememberInfiniteTransition(label = "night")
        val flight by t.animateFloat(0f, 1f, infiniteRepeatable(tween(17_000, easing = LinearEasing)), label = "bats")
        val flap by t.animateFloat(0f, 1f, infiniteRepeatable(tween(420, easing = LinearEasing), RepeatMode.Reverse), label = "flap")
        val twinkle by t.animateFloat(0f, 1f, infiniteRepeatable(tween(2400), RepeatMode.Reverse), label = "tw")
        val k = if (strong) 1f else 0.45f
        Canvas(modifier) {
            // звёзды
            for (i in 0 until 46) {
                val a = (0.35f + 0.65f * abs(sin((twinkle + rnd(i, 30)) * PI)).toFloat()) * k
                drawCircle(Color.White.copy(alpha = a * 0.8f), (0.6f + rnd(i, 31) * 1.1f).dp.toPx(),
                    Offset(rnd(i, 32) * size.width, rnd(i, 33) * size.height * 0.62f))
            }
            // луна с кратерами
            val mc = Offset(size.width * 0.78f, size.height * (if (strong) 0.14f else 0.12f))
            val mr = size.width * (if (strong) 0.13f else 0.09f)
            drawCircle(Brush.radialGradient(listOf(P.Moon.copy(alpha = 0.32f * k), Color.Transparent), mc, mr * 2.6f), mr * 2.6f, mc)
            drawCircle(P.Moon.copy(alpha = 0.92f * k + 0.05f), mr, mc)
            drawCircle(Color(0xFFCFC6A2).copy(alpha = 0.7f * k), mr * 0.22f, mc + Offset(-mr * 0.3f, -mr * 0.2f))
            drawCircle(Color(0xFFCFC6A2).copy(alpha = 0.6f * k), mr * 0.14f, mc + Offset(mr * 0.35f, mr * 0.25f))
            drawCircle(Color(0xFFCFC6A2).copy(alpha = 0.5f * k), mr * 0.1f, mc + Offset(-mr * 0.05f, mr * 0.45f))
            castle(k)
            // летучие мыши
            for (b in 0 until 3) {
                val p = (flight + b * 0.33f) % 1f
                val x = -40f + p * (size.width + 80f)
                val y = size.height * (0.18f + b * 0.09f) + sin(p * 6 * PI + b).toFloat() * 14.dp.toPx()
                bat(Offset(x, y), (if (b == 1) 10f else 14f).dp.toPx(), if (b == 1) 1f - flap else flap, Color(0xFF07050C).copy(alpha = 0.55f + 0.45f * k))
            }
        }
    }

    /** Силуэт замка по нижнему краю: стена с зубцами, башни с конусными крышами и шпилями, горящие окна. */
    private fun DrawScope.castle(k: Float) {
        val w = size.width
        val base = size.height
        val u = w / 24f
        val sil = Color(0xFF06040A).copy(alpha = 0.55f + 0.45f * k)
        val wall = Path().apply {
            moveTo(0f, base)
            lineTo(0f, base - 3.2f * u)
            var x = 0f
            while (x < w) {   // зубцы стены
                lineTo(x, base - 3.8f * u); lineTo(x + 0.5f * u, base - 3.8f * u); lineTo(x + 0.5f * u, base - 3.2f * u); lineTo(x + u, base - 3.2f * u)
                x += u
            }
            lineTo(w, base); close()
        }
        drawPath(wall, sil)
        data class Tower(val cx: Float, val wd: Float, val h: Float)
        val towers = listOf(Tower(3f, 2.2f, 8f), Tower(8.5f, 1.6f, 6f), Tower(13f, 3f, 11f), Tower(17.5f, 1.8f, 7f), Tower(21f, 2.4f, 9f))
        for ((i, t) in towers.withIndex()) {
            val l = (t.cx - t.wd / 2) * u
            val r = (t.cx + t.wd / 2) * u
            val top = base - t.h * u
            drawRect(sil, Offset(l, top), Size(r - l, t.h * u))
            val roof = Path().apply {
                moveTo(l - 0.35f * u, top); lineTo((l + r) / 2, top - t.wd * 1.6f * u); lineTo(r + 0.35f * u, top); close()
            }
            drawPath(roof, sil)
            drawLine(sil, Offset((l + r) / 2, top - t.wd * 1.6f * u), Offset((l + r) / 2, top - t.wd * 1.6f * u - 0.9f * u), 2f)
            // окна: тёплый свет, часть «не горит»
            for (j in 0 until (t.h / 2.5f).toInt()) {
                if (rnd(i * 7 + j, 40) < 0.35f) continue
                val wy = top + (1.2f + j * 2.4f) * u
                drawRoundRect(Color(0xFFFFC560).copy(alpha = 0.55f * k + 0.1f), Offset((l + r) / 2 - 0.22f * u, wy),
                    Size(0.44f * u, 0.8f * u), androidx.compose.ui.geometry.CornerRadius(0.22f * u))
            }
        }
    }

    /** Летучая мышь: тело и крылья-«зубцы», `flap` 0..1 — взмах. */
    private fun DrawScope.bat(c: Offset, span: Float, flap: Float, color: Color) {
        val up = (flap - 0.5f) * span * 0.6f
        val p = Path().apply {
            moveTo(c.x, c.y)
            quadraticTo(c.x - span * 0.4f, c.y - span * 0.25f - up, c.x - span, c.y - up)
            lineTo(c.x - span * 0.78f, c.y + span * 0.12f - up * 0.5f)
            lineTo(c.x - span * 0.6f, c.y + span * 0.02f - up * 0.4f)
            lineTo(c.x - span * 0.42f, c.y + span * 0.18f - up * 0.3f)
            lineTo(c.x - span * 0.2f, c.y + span * 0.08f)
            lineTo(c.x, c.y + span * 0.22f)
            lineTo(c.x + span * 0.2f, c.y + span * 0.08f)
            lineTo(c.x + span * 0.42f, c.y + span * 0.18f - up * 0.3f)
            lineTo(c.x + span * 0.6f, c.y + span * 0.02f - up * 0.4f)
            lineTo(c.x + span * 0.78f, c.y + span * 0.12f - up * 0.5f)
            lineTo(c.x + span, c.y - up)
            quadraticTo(c.x + span * 0.4f, c.y - span * 0.25f - up, c.x, c.y)
            close()
        }
        drawPath(p, color)
        drawCircle(color, span * 0.14f, Offset(c.x, c.y - span * 0.05f))
        // ушки
        drawLine(color, Offset(c.x - span * 0.07f, c.y - span * 0.12f), Offset(c.x - span * 0.1f, c.y - span * 0.26f), span * 0.06f)
        drawLine(color, Offset(c.x + span * 0.07f, c.y - span * 0.12f), Offset(c.x + span * 0.1f, c.y - span * 0.26f), span * 0.06f)
    }

    /** Пузырь сообщения: у «Кровавой луны» под частью исходящих — подтёк, у входящих — багровая кайма;
     *  у «Ночи в замке» — серебряная кайма. */
    fun Modifier.bubbleDecor(out: Boolean, key: String): Modifier = when (P.style) {
        "blood" -> this.drawBehind {
            val h = key.hashCode()
            if (out && (h and 3) != 0) {
                val x = size.width * (0.25f + ((h ushr 3) and 0xff) / 255f * 0.5f)
                drip(Offset(x, size.height - 2f), 6.5.dp.toPx(), (7f + ((h ushr 11) and 7)).dp.toPx())
            }
            if (!out) drawPath(bubblePath(out), P.Blood.copy(alpha = 0.45f), style = Stroke(1.dp.toPx()))
        }
        "night" -> this.drawBehind {
            drawPath(bubblePath(out), P.Silver.copy(alpha = if (out) 0.35f else 0.22f), style = Stroke(1.dp.toPx()))
        }
        else -> this
    }

    /** Контур пузыря той же формы, что у Surface в чате: острый угол — у «хвоста». */
    private fun DrawScope.bubblePath(out: Boolean): Path {
        val r = androidx.compose.ui.geometry.CornerRadius(18.dp.toPx())
        val t = androidx.compose.ui.geometry.CornerRadius(4.dp.toPx())
        return Path().apply {
            addRoundRect(androidx.compose.ui.geometry.RoundRect(0f, 0f, size.width, size.height,
                topLeftCornerRadius = r, topRightCornerRadius = r,
                bottomRightCornerRadius = if (out) t else r, bottomLeftCornerRadius = if (out) r else t))
        }
    }
}
