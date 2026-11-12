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
 * Оформление вампирских и фейских тем — рисуется на Canvas, без картинок:
 *  • «Кровавая луна»: кровь стекает с нижнего края панелей (капли срываются и падают), брызги на фоне,
 *    подтёки под исходящими сообщениями, кровавая луна на экране звонка;
 *  • «Ночь в замке»: готические арки под панелями, звёзды, луна, силуэт замка с окнами и летучие мыши;
 *  • «Лесная фея»: лиана с листьями и светящимися ягодами под панелью, светлячки, трава и мухоморы;
 *  • «Цветочная фея»: гирлянда цветов под панелью, падающая пыльца-блёстки, бабочки, цветы по низу.
 * При обычных темах все функции ничего не рисуют.
 */
object Decor {
    /** Детерминированный «случайный» ряд: оформление не прыгает при перерисовке. */
    private fun rnd(i: Int, salt: Int = 0): Float {
        val x = sin((i * 12.9898 + salt * 78.233)) * 43758.5453
        return (x - kotlin.math.floor(x)).toFloat()
    }

    /** Значок у названия приложения. */
    val glyph: String get() = when (P.style) { "blood" -> "🩸 "; "night" -> "🦇 "; "forest" -> "🍄 "; "bloom" -> "🌸 "; else -> "" }

    /** Нижний край верхней панели: подтёки крови / готические арки. */
    @Composable
    fun TopEdge(modifier: Modifier = Modifier) {
        when (P.style) {
            "blood" -> Drips(modifier.fillMaxWidth().height(34.dp), P.Side)
            "night" -> Arches(modifier.fillMaxWidth().height(12.dp), P.Side)
            "forest" -> Vine(modifier.fillMaxWidth().height(24.dp))
            "bloom" -> Garland(modifier.fillMaxWidth().height(22.dp))
        }
    }

    @Composable
    private fun Vine(modifier: Modifier) {
        val t = rememberInfiniteTransition(label = "vine")
        val ph by t.animateFloat(0f, (2 * PI).toFloat(), infiniteRepeatable(tween(4000, easing = LinearEasing)), label = "ph")
        Canvas(modifier) {
            val y0 = 6.dp.toPx()
            val amp = 4.dp.toPx()
            val path = Path().apply {
                moveTo(0f, y0)
                var x = 0f
                while (x <= size.width) { lineTo(x, y0 + sin(x / 38.dp.toPx()) * amp); x += 4f }
            }
            drawPath(path, Color(0xFF2F5E3A), style = Stroke(2.dp.toPx()))
            var x = 12.dp.toPx()
            var i = 0
            while (x < size.width) {
                val y = y0 + sin(x / 38.dp.toPx()) * amp
                val dir = if (i % 2 == 0) 1f else -1f
                leaf(Offset(x, y), (9f + rnd(i, 3) * 5f).dp.toPx(), 1.2f * dir + 0.3f * sin(ph + i), Color(0xFF4F9A4A))
                if (rnd(i, 5) > 0.7f) {
                    val glow = 0.6f + 0.4f * sin(ph * 2 + i)
                    drawCircle(P.Firefly.copy(alpha = 0.18f * glow), 5.dp.toPx(), Offset(x + 5.dp.toPx(), y + 7.dp.toPx()))
                    drawCircle(P.Firefly.copy(alpha = glow), 2.2.dp.toPx(), Offset(x + 5.dp.toPx(), y + 7.dp.toPx()))
                }
                x += (26f + rnd(i, 4) * 20f).dp.toPx()
                i++
            }
        }
    }

    @Composable
    private fun Garland(modifier: Modifier) {
        Canvas(modifier) {
            val span = 150.dp.toPx()
            var x0 = 0f
            var g = 0
            while (x0 < size.width) {
                val path = Path().apply {
                    moveTo(x0, 2.dp.toPx())
                    for (k in 1..16) { val u = k / 16f; lineTo(x0 + u * span, 2.dp.toPx() + sin(u * PI).toFloat() * 10.dp.toPx()) }
                }
                drawPath(path, Color(0xFF8FC99B), style = Stroke(1.2.dp.toPx()))
                listOf(0.2f, 0.5f, 0.8f).forEachIndexed { j, k ->
                    flower(Offset(x0 + k * span, 4.dp.toPx() + sin(k * PI).toFloat() * 10.dp.toPx()),
                        (6f + rnd(g * 3 + j, 2) * 2f).dp.toPx(), P.Petals[(g + j) % 3])
                }
                x0 += span
                g++
            }
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
            "forest" -> Forest(modifier, strong = false)
            "bloom" -> Meadow(modifier, strong = false)
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
            "forest" -> Forest(modifier, strong = true)
            "bloom" -> Meadow(modifier, strong = true)
        }
    }

    /** Лес ночью: светлячки кружат и мерцают; по низу (strong) — трава, мухоморы. */
    @Composable
    private fun Forest(modifier: Modifier, strong: Boolean) {
        val t = rememberInfiniteTransition(label = "forest")
        val tm by t.animateFloat(0f, 1000f, infiniteRepeatable(tween(1_000_000, easing = LinearEasing)), label = "t")
        Canvas(modifier) {
            val k = if (strong) 1f else 0.6f
            if (strong) {
                drawCircle(Brush.radialGradient(listOf(Color(0xFF9BE564).copy(alpha = 0.10f), Color.Transparent),
                    Offset(size.width / 2, size.height * 0.28f), size.width * 0.6f), size.width * 0.6f, Offset(size.width / 2, size.height * 0.28f))
            }
            val n = if (strong) 26 else 12
            for (i in 0 until n) {
                val bx = rnd(i, 50) * size.width
                val by = (0.1f + rnd(i, 51) * 0.75f) * size.height
                val x = bx + sin(tm * 0.3f + rnd(i, 52) * 9f) * 30.dp.toPx()
                val y = by + kotlin.math.cos(tm * 0.45f + rnd(i, 53) * 9f) * 20.dp.toPx()
                val glow = (0.25f + 0.75f * (sin(tm * 1.7f + rnd(i, 54) * 9f) * 0.5f + 0.5f)) * k
                firefly(Offset(x, y), glow)
            }
            if (strong) {
                val h0 = size.height
                var i = 0
                var x = 0f
                while (x < size.width) {
                    val gh = (10f + rnd(i, 41) * 22f).dp.toPx()
                    val sway = sin(tm * 1.2f + i * 0.3f) * 3.dp.toPx()
                    drawLine(Color(0xFF1F4A2C), Offset(x, h0), Offset(x + sway, h0 - gh), 2.dp.toPx())
                    x += (6f + rnd(i, 40) * 4f).dp.toPx(); i++
                }
                listOf(0.1f, 0.27f, 0.78f, 0.9f).forEachIndexed { j, fx ->
                    mushroom(Offset(size.width * fx, h0 - 2.dp.toPx()), (22f + rnd(j, 42) * 14f).dp.toPx())
                }
            }
        }
    }

    /** Луг: пыльца-блёстки падают и мерцают, бабочки; по низу (strong) — цветы на стеблях. */
    @Composable
    private fun Meadow(modifier: Modifier, strong: Boolean) {
        val t = rememberInfiniteTransition(label = "meadow")
        val tm by t.animateFloat(0f, 1000f, infiniteRepeatable(tween(1_000_000, easing = LinearEasing)), label = "t")
        Canvas(modifier) {
            val k = if (strong) 1f else 0.6f
            val n = if (strong) 30 else 14
            for (i in 0 until n) {
                val speed = (12f + rnd(i, 70) * 18f).dp.toPx()
                val y = (tm * speed + rnd(i, 71) * size.height) % size.height
                val x = rnd(i, 72) * size.width + sin(tm * 0.7f + i) * 10.dp.toPx()
                val tw = sin(tm * 3f + rnd(i, 73) * 9f) * 0.5f + 0.5f
                sparkle(Offset(x, y), (2f + tw * 3f).dp.toPx(), P.Gold.copy(alpha = (0.35f + 0.6f * tw) * k))
            }
            for (b in 0 until (if (strong) 3 else 2)) {
                val ph = (tm / 20f + b * 0.37f) % 1f
                val x = -20.dp.toPx() + ph * (size.width + 40.dp.toPx())
                val y = size.height * (0.2f + b * 0.22f) + sin(ph * 8 * PI).toFloat() * 24.dp.toPx()
                butterfly(Offset(x, y), 12.dp.toPx(), sin(tm * 9f + b) * 0.5f + 0.5f, P.Petals[b % 3].copy(alpha = 0.9f))
            }
            if (strong) {
                listOf(0.06f, 0.15f, 0.24f, 0.76f, 0.85f, 0.94f).forEachIndexed { i, fx ->
                    val x = size.width * fx
                    val h = (40f + rnd(i, 60) * 50f).dp.toPx()
                    val top = Offset(x + sin(tm + i) * 2.dp.toPx(), size.height - h)
                    drawLine(Color(0xFF8FC99B), Offset(x, size.height), top, 2.dp.toPx())
                    leaf(Offset(x, size.height - h * 0.4f), 10.dp.toPx(), (if (i % 2 == 0) -2.3f else -0.8f), Color(0xFF8FC99B))
                    flower(top, (10f + rnd(i, 61) * 5f).dp.toPx(), P.Petals[i % 3])
                }
            }
        }
    }

    private fun DrawScope.firefly(c: Offset, glow: Float) {
        drawCircle(P.Firefly.copy(alpha = 0.06f * glow), 10.dp.toPx(), c)
        drawCircle(P.Firefly.copy(alpha = 0.16f * glow), 5.dp.toPx(), c)
        drawCircle(P.Firefly.copy(alpha = (0.4f + 0.6f * glow).coerceAtMost(1f)), 2.dp.toPx(), c)
    }

    /** Мухомор: ножка, красная шляпка в белых точках. */
    private fun DrawScope.mushroom(base: Offset, s: Float) {
        val stem = Path().apply {
            moveTo(base.x - s * 0.16f, base.y); lineTo(base.x + s * 0.16f, base.y)
            lineTo(base.x + s * 0.11f, base.y - s * 0.75f); lineTo(base.x - s * 0.11f, base.y - s * 0.75f); close()
        }
        drawPath(stem, Color(0xFFF1E6D2))
        drawArc(Color(0xFFC82A22), 180f, 180f, true, Offset(base.x - s * 0.6f, base.y - s * 1.2f), Size(s * 1.2f, s * 1.0f))
        for ((dx, dy, r) in listOf(Triple(-0.3f, -0.95f, 0.08f), Triple(0.05f, -1.1f, 0.07f), Triple(0.32f, -0.9f, 0.06f), Triple(-0.05f, -0.85f, 0.05f)))
            drawCircle(Color.White.copy(alpha = 0.9f), r * s, Offset(base.x + dx * s, base.y + dy * s))
    }

    /** Лист: две дуги, `ang` — направление (радианы). */
    private fun DrawScope.leaf(at: Offset, len: Float, ang: Float, color: Color) {
        val d = Offset(kotlin.math.cos(ang), sin(ang))
        val n = Offset(-d.y, d.x)
        val p = Path().apply {
            moveTo(at.x, at.y)
            for (k in 1..8) { val u = k / 8f; val o = at + d * (len * u) + n * (sin(u * PI).toFloat() * len * 0.3f); lineTo(o.x, o.y) }
            for (k in 7 downTo 0) { val u = k / 8f; val o = at + d * (len * u) - n * (sin(u * PI).toFloat() * len * 0.3f); lineTo(o.x, o.y) }
            close()
        }
        drawPath(p, color)
    }

    /** Цветок: пять лепестков и серединка. */
    private fun DrawScope.flower(c: Offset, r: Float, color: Color) {
        for (k in 0 until 5) {
            val a = k / 5f * 2 * PI.toFloat() - PI.toFloat() / 2
            drawCircle(color, r * 0.5f, c + Offset(kotlin.math.cos(a), sin(a)) * (r * 0.62f))
        }
        drawCircle(Color(0xFFFFE08A), r * 0.38f, c)
    }

    /** Бабочка: две пары крыльев; `flap` 0..1 — взмах. */
    private fun DrawScope.butterfly(c: Offset, s: Float, flap: Float, color: Color) {
        val wx = 0.35f + 0.65f * flap
        for (side in listOf(-1f, 1f)) {
            drawOval(color, Offset(c.x + side * 0.55f * s * wx - s * 0.5f * wx, c.y - s * 0.8f), Size(s * wx, s * 0.9f))
            drawOval(color.copy(alpha = color.alpha * 0.85f), Offset(c.x + side * 0.4f * s * wx - s * 0.35f * wx, c.y), Size(s * 0.7f * wx, s * 0.6f))
        }
        drawLine(Color(0xFF4A3550), Offset(c.x, c.y - s * 0.5f), Offset(c.x, c.y + s * 0.55f), s * 0.12f)
    }

    /** Блёстка: четырёхлучевая звёздочка. */
    private fun DrawScope.sparkle(c: Offset, r: Float, color: Color) {
        val p = Path().apply {
            for (k in 0 until 8) {
                val a = k / 8f * 2 * PI.toFloat()
                val rr = if (k % 2 == 0) r else r * 0.28f
                val x = c.x + kotlin.math.cos(a) * rr; val y = c.y + sin(a) * rr
                if (k == 0) moveTo(x, y) else lineTo(x, y)
            }
            close()
        }
        drawPath(p, color)
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
        "forest" -> this.drawBehind {
            if (key.hashCode() % 3 == 0)
                leaf(Offset(if (out) 6.dp.toPx() else size.width - 6.dp.toPx(), 4.dp.toPx()), 12.dp.toPx(),
                    if (out) -2.4f else -0.7f, Color(0xFF6FB85A))
        }
        "bloom" -> this.drawBehind {
            if (!out) drawPath(bubblePath(out), P.Petals[0].copy(alpha = 0.45f), style = Stroke(1.dp.toPx()))
            if (key.hashCode() % 3 == 0) sparkle(Offset(size.width - 6.dp.toPx(), 6.dp.toPx()), 6.dp.toPx(), P.Gold)
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
