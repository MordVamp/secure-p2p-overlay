package org.p2poverlay.android.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.CallEnd
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material.icons.filled.Hearing
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.delay
import org.json.JSONObject
import org.p2poverlay.android.AppViewModel
import org.p2poverlay.android.CallService

/** Действие, которому нужен микрофон: спросить разрешение (отказ не мешает — будет слышно собеседника). */
@Composable
fun rememberWithMic(action: (String) -> Unit): (String) -> Unit {
    val ctx = LocalContext.current
    var pending by remember { androidx.compose.runtime.mutableStateOf<String?>(null) }
    val perm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        pending?.let(action); pending = null
    }
    return { arg ->
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) action(arg)
        else { pending = arg; perm.launch(Manifest.permission.RECORD_AUDIO) }
    }
}

private fun JSONObject.peersList(): List<JSONObject> =
    optJSONArray("peers")?.let { a -> (0 until a.length()).map { a.getJSONObject(it) } } ?: emptyList()

/** Полоса «идёт звонок» над свёрнутым экраном звонка. */
@Composable
fun CallBar(vm: AppViewModel) {
    val c = vm.call ?: return
    val dur = c.optLong("duration_s", -1)
    Row(Modifier.fillMaxWidth().background(P.Green).statusBarsPadding().clickable { vm.callMinimized = false }
        .padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(Icons.Filled.Call, null, tint = Color.White, modifier = Modifier.size(18.dp))
        Text("  ${vm.callTitle(c)}", color = Color.White, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f),
            maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(if (dur >= 0) "%d:%02d".format(dur / 60, dur % 60) else "вызов…", color = Color.White)
    }
}

@Composable
fun CallScreen(vm: AppViewModel) {
    val c = vm.call ?: return
    BackHandler { vm.callMinimized = true }
    val sid = c.optString("id")
    val state = c.optString("state")
    val incoming = state == "ringing" && !c.optBoolean("outgoing")
    val group = c.optJSONObject("group")
    val peers = c.peersList().filter { it.optString("state") != "ended" || state == "ended" }
    val accept = rememberWithMic { vm.acceptCall(it) }
    val audio by CallService.audio.collectAsState()
    var myLevel by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(audio) { while (true) { myLevel = audio?.micLevel ?: 0f; delay(120) } }

    val dur = c.optLong("duration_s", -1)
    val status = when {
        state == "ended" -> when (c.optString("end_reason")) {
            "busy" -> "Абонент занят"; "rejected" -> "Звонок отклонён"; "no_answer" -> "Нет ответа"
            "missed" -> "Пропущенный звонок"; "unreachable" -> "Абонент недоступен"; else -> "Звонок завершён"
        }
        incoming -> if (group != null) "Групповой звонок" else "Входящий звонок"
        state == "ringing" -> "Вызов…"
        dur >= 0 -> "%d:%02d".format(dur / 60, dur % 60)
        else -> "Соединение…"
    }

    Box(Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(P.Side, P.Rail)))) {
        Decor.CallBackdrop(Modifier.fillMaxSize())
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                if (state != "ended") IconButton(onClick = { vm.callMinimized = true }) {
                    Icon(Icons.Filled.KeyboardArrowDown, "Свернуть", tint = P.Text)
                }
                Text("Через туннель · E2E · ключ звонка", color = P.Muted, fontSize = 12.sp, modifier = Modifier.weight(1f),
                    textAlign = TextAlign.Center)
                Spacer(Modifier.size(48.dp))
            }
            Spacer(Modifier.height(36.dp))
            if (group == null) {
                val peer = peers.firstOrNull()?.optString("peer").orEmpty()
                Speaking(level = peers.firstOrNull()?.optDouble("level", 0.0)?.toFloat() ?: 0f, ringing = state == "ringing", size = 132.dp) {
                    Avatar(peer, vm.nameOf(peer), 120.dp)
                }
            } else {
                Text("👥 ${group.optString("name")}", color = P.Text, fontSize = 15.sp)
                Spacer(Modifier.height(12.dp))
                val me = vm.myId
                val tiles = listOf(me to myLevel) + peers.map { it.optString("peer") to it.optDouble("level", 0.0).toFloat() }
                tiles.chunked(3).forEach { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(16.dp), modifier = Modifier.padding(vertical = 8.dp)) {
                        row.forEach { (id, lvl) ->
                            val pState = peers.firstOrNull { it.optString("peer") == id }?.optString("state") ?: "active"
                            Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.size(width = 96.dp, height = 146.dp)) {
                                Speaking(lvl, ringing = pState == "ringing", size = 76.dp) { Avatar(id, if (id == me) "Вы" else vm.nameOf(id), 68.dp) }
                                Text(if (id == me) "Вы" else vm.nameOf(id), color = P.Text, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                if (pState == "ringing") Text("вызов…", color = P.Muted, fontSize = 11.sp)
                            }
                        }
                    }
                }
            }
            Spacer(Modifier.height(20.dp))
            Text(vm.callTitle(c), color = P.Text, fontSize = 26.sp, fontWeight = FontWeight.SemiBold, maxLines = 1,
                overflow = TextOverflow.Ellipsis)
            Text(status, color = if (state == "ended") P.Red else P.Muted, fontSize = 16.sp, modifier = Modifier.padding(top = 6.dp))
            if (state == "active") Quality(c)
            Spacer(Modifier.weight(1f))
            when {
                state == "ended" -> {}
                incoming -> Row(horizontalArrangement = Arrangement.SpaceEvenly, modifier = Modifier.fillMaxWidth()) {
                    RoundButton(Icons.Filled.CallEnd, "Отклонить", P.Red) { vm.endCall(sid, reject = true) }
                    RoundButton(Icons.Filled.Call, "Ответить", P.Green) { accept(sid) }
                }
                else -> Row(horizontalArrangement = Arrangement.SpaceEvenly, modifier = Modifier.fillMaxWidth()) {
                    RoundButton(if (vm.muted) Icons.Filled.MicOff else Icons.Filled.Mic, if (vm.muted) "Микрофон выкл." else "Микрофон",
                        if (vm.muted) P.Text else P.Card, iconTint = if (vm.muted) P.Rail else P.Text) { vm.toggleMute() }
                    RoundButton(Icons.Filled.CallEnd, "Завершить", P.Red) { vm.endCall(sid) }
                    RoundButton(if (vm.speaker) Icons.AutoMirrored.Filled.VolumeUp else Icons.Filled.Hearing, if (vm.speaker) "Динамик" else "У уха",
                        if (vm.speaker) P.Text else P.Card, iconTint = if (vm.speaker) P.Rail else P.Text) { vm.toggleSpeaker() }
                }
            }
            Spacer(Modifier.height(28.dp))
        }
    }
}

/** Качество: среднее по собеседникам — MOS (E-модель), задержка «рот — ухо», слышимые потери. */
@Composable
private fun Quality(c: JSONObject) {
    val qs = c.peersList().filter { it.optString("state") == "active" }.mapNotNull { it.optJSONObject("quality") }
    val mos = qs.mapNotNull { q -> q.optDouble("mos").takeIf { !it.isNaN() } }
    if (mos.isEmpty()) return
    val m2e = qs.mapNotNull { q -> q.optDouble("mouth_to_ear_ms").takeIf { !it.isNaN() } }.average()
    val loss = qs.map { it.optDouble("loss_audible_pct", 0.0) }.average()
    val avg = mos.average()
    val color = when { avg >= 4.0 -> P.Green; avg >= 3.6 -> P.Yellow; else -> P.Red }
    Row(Modifier.padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Pill("MOS %.1f".format(avg), color)
        Pill("%.0f мс".format(m2e), P.Muted)
        Pill("потери %.1f%%".format(loss), if (loss < 1) P.Muted else P.Yellow)
    }
}

/** Аватар с кольцом «говорит» (по громкости) или пульсацией вызова. */
@Composable
private fun Speaking(level: Float, ringing: Boolean, size: Dp, content: @Composable () -> Unit) {
    val t = rememberInfiniteTransition(label = "ring")
    val pulse by t.animateFloat(1f, 1.18f, infiniteRepeatable(tween(900), RepeatMode.Reverse), label = "p")
    val speaking = level > 0.02f
    Box(contentAlignment = Alignment.Center, modifier = Modifier.size(size + 24.dp)) {
        if (ringing) Box(Modifier.size(size).scale(pulse).clip(CircleShape).background(P.Accent.copy(alpha = 0.18f)))
        Box(Modifier.size(size).clip(CircleShape)
            .border(if (speaking) 3.dp else 0.dp, if (speaking) P.Green.copy(alpha = (0.5f + level * 4).coerceAtMost(1f)) else Color.Transparent, CircleShape),
            contentAlignment = Alignment.Center) { content() }
    }
}

@Composable
private fun RoundButton(icon: ImageVector, label: String, bg: Color, iconTint: Color = Color.White, onClick: () -> Unit) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(68.dp).clip(CircleShape).background(bg).clickable(onClick = onClick), contentAlignment = Alignment.Center) {
            Icon(icon, label, tint = iconTint, modifier = Modifier.size(30.dp))
        }
        Text(label, color = P.Muted, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
    }
}
