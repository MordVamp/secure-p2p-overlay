package org.p2poverlay.android.ui

import android.content.Intent
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.RestartAlt
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.p2poverlay.android.AppViewModel
import org.p2poverlay.android.Invite
import org.p2poverlay.android.NodeService
import org.p2poverlay.android.PowerMode
import org.p2poverlay.android.RunState

@Composable
fun NodeScreen(vm: AppViewModel, onScan: () -> Unit, onInvite: (Invite) -> Unit) {
    val run by vm.runState.collectAsState()
    val ctx = LocalContext.current
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        // ── состояние ──
        SectionCard("Узел на телефоне") {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                when (val r = run) {
                    is RunState.Running -> Pill("работает", P.Green)
                    RunState.Starting -> Pill("запуск…", P.Yellow)
                    RunState.Stopped -> Pill("остановлен", P.Muted)
                    is RunState.Failed -> Pill("ошибка", P.Red)
                }
                Text(vm.status?.optString("addr") ?: "", color = P.Muted, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
            }
            (run as? RunState.Failed)?.let { Text(it.reason, color = P.Red, fontSize = 13.sp) }
            vm.status?.let { st ->
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Pill(vm.prefs.mode.title, P.Accent)
                    Pill(if (st.optBoolean("relay", true)) "ретранслятор" else "только клиент", if (st.optBoolean("relay", true)) P.Green else P.Yellow)
                    val q = st.optJSONObject("outbox")?.let { o -> o.keys().asSequence().sumOf { o.optInt(it) } } ?: 0
                    if (q > 0) Pill("в почтовых ящиках: $q", P.Yellow)
                }
            }
            Text("Это полноценный узел оверлея (тот же код на Rust, что и на компьютере): Kademlia DHT, туннели, " +
                "сквозное шифрование. Компьютер для него — просто ещё один пир.", color = P.Muted, fontSize = 13.sp)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (run is RunState.Running || run is RunState.Starting) {
                    Button(onClick = vm::stopNode, colors = ButtonDefaults.buttonColors(containerColor = P.Red.copy(alpha = 0.8f))) {
                        Icon(Icons.Filled.Stop, null); Text(" Остановить")
                    }
                    FilledTonalButton(onClick = vm::restartNode) { Icon(Icons.Filled.RestartAlt, null); Text(" Перезапуск") }
                } else {
                    Button(onClick = vm::startNode) { Icon(Icons.Filled.PlayArrow, null); Text(" Запустить") }
                }
            }
        }

        // ── подключение к компьютеру ──
        SectionCard("Подключиться к компьютеру") {
            Text("На компьютере: P2P Overlay → «Подключение» → включите «Локальная сеть» и ретрансляторы, " +
                "запустите узел и отсканируйте QR. Телефон и компьютер должны быть в одной сети Wi-Fi.",
                color = P.Muted, fontSize = 13.sp)
            Button(onClick = onScan, modifier = Modifier.fillMaxWidth()) {
                Icon(Icons.Filled.QrCodeScanner, null); Text("  Сканировать QR")
            }
            var link by rememberSaveable { mutableStateOf("") }
            Row(verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(link, { link = it }, label = { Text("или ссылка p2p-overlay://…") }, singleLine = true,
                    modifier = Modifier.weight(1f))
                TextButton(enabled = Invite.parse(link) != null, onClick = { Invite.parse(link)?.let(onInvite); link = "" }) {
                    Text("OK")
                }
            }
        }

        // ── профиль ──
        if (vm.status != null) SectionCard("Мой профиль") {
            var alias by rememberSaveable { mutableStateOf(vm.myAlias) }
            Row(verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(alias, { alias = it.filter { c -> c.isLetterOrDigit() || c in "_-." } },
                    label = { Text("Псевдоним") }, prefix = { Text("@") }, singleLine = true, modifier = Modifier.weight(1f))
                TextButton(enabled = alias.isNotBlank(), onClick = { vm.registerAlias(alias) }) { Text("Опубликовать") }
            }
            Text("Подписанная запись в DHT: собеседники найдут вас по @${alias.ifBlank { "имя" }}; " +
                "первый зарегистрировавший имя — владелец.", color = P.Muted, fontSize = 12.sp)
            val myLink = vm.myLink
            if (myLink.isNotEmpty()) {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { QrImage(myLink, 200.dp) }
                Text(vm.myId, fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = P.Muted)
                Row {
                    IconButton(onClick = { ctx.getSystemService(android.content.ClipboardManager::class.java)
                            .setPrimaryClip(android.content.ClipData.newPlainText("NodeID", vm.myId)); vm.toast("NodeID скопирован") }) {
                        Icon(Icons.Filled.ContentCopy, "Копировать")
                    }
                    IconButton(onClick = {
                        ctx.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain")
                            .putExtra(Intent.EXTRA_TEXT, myLink), "Ссылка на мой узел"))
                    }) { Icon(Icons.Filled.Share, "Поделиться") }
                }
            }
        }

        // ── настройки ──
        SectionCard("Настройки узла") {
            val p = vm.prefs
            var bootstrap by rememberSaveable { mutableStateOf(p.bootstrap) }
            var port by rememberSaveable { mutableStateOf(p.listenPort.toString()) }
            var channel by rememberSaveable { mutableStateOf(p.channel) }
            var lan by rememberSaveable { mutableStateOf(p.lan) }
            var auto by rememberSaveable { mutableStateOf(p.autostart) }
            var notify by rememberSaveable { mutableStateOf(p.notifyMessages) }
            var mode by rememberSaveable { mutableStateOf(p.powerMode) }
            var clientOnly by rememberSaveable { mutableStateOf(p.clientOnly) }
            Text("Режим энергопотребления", color = P.Muted, fontSize = 13.sp)
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                PowerMode.entries.forEachIndexed { i, m ->
                    SegmentedButton(selected = mode == m.id, onClick = { mode = m.id },
                        shape = SegmentedButtonDefaults.itemShape(i, PowerMode.entries.size)) { Text(m.title) }
                }
            }
            Text(when (PowerMode.of(mode)) {
                PowerMode.ECO -> "Только клиент, процессор может засыпать, DHT обновляется раз в 10 мин, почтовые ящики — раз в 5 мин. " +
                    "Входящие приходят, но узел не помогает сети."
                PowerMode.NORMAL -> "Ретранслятор для чужих туннелей, без постоянной блокировки сна, обычные интервалы."
                PowerMode.MAX -> "Ретранслятор с блокировкой сна (всегда на связи), пул из 2 туннелей к собеседнику " +
                    "для мгновенного переключения, почтовые ящики — раз в 30 с. Расходует батарею."
            }, color = P.Muted, fontSize = 12.sp)
            SwitchRow("Только клиент", "не ретранслировать чужой трафик (в «Экономе» — всегда)",
                clientOnly || mode == PowerMode.ECO.id, enabled = mode != PowerMode.ECO.id) { clientOnly = it }
            OutlinedTextField(bootstrap, { bootstrap = it }, label = { Text("Bootstrap-пиры (host:port, …)") },
                singleLine = true, modifier = Modifier.fillMaxWidth())
            OutlinedTextField(port, { port = it.filter(Char::isDigit).take(5) }, label = { Text("P2P-порт") },
                singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
            Text("Канал между узлами (должен совпадать во всей сети)", color = P.Muted, fontSize = 13.sp)
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                listOf("tls" to "TLS 1.3", "ake" to "Своё рукопожатие").forEachIndexed { i, (v, l) ->
                    SegmentedButton(selected = channel == v, onClick = { channel = v },
                        shape = SegmentedButtonDefaults.itemShape(i, 2)) { Text(l) }
                }
            }
            SwitchRow("Доступен в локальной сети", "сообщать пирам Wi-Fi-адрес ${NodeService.wifiIp(ctx) ?: "—"}", lan) { lan = it }
            SwitchRow("Запускать узел при открытии", null, auto) { auto = it }
            SwitchRow("Уведомления о сообщениях", null, notify) { notify = it }
            Button(onClick = {
                p.bootstrap = bootstrap.trim(); p.listenPort = port.toIntOrNull()?.coerceIn(1024, 65535) ?: 7600
                p.channel = channel; p.lan = lan; p.autostart = auto; p.notifyMessages = notify
                p.powerMode = mode; p.clientOnly = clientOnly
                vm.toast("Сохранено — перезапуск узла")
                vm.restartNode()
            }) { Text("Применить и перезапустить") }
        }

        // ── приватность и вид (применяются сразу) ──
        SectionCard("Приватность и вид") {
            val p = vm.prefs
            var typing by remember { mutableStateOf(p.sendTyping) }
            var receipts by remember { mutableStateOf(p.sendReceipts) }
            var bio by remember { mutableStateOf(p.biometricLock) }
            SwitchRow("Сообщать «печатает…»", "одно короткое сообщение раз в 4 с по уже открытому туннелю", typing) {
                typing = it; p.sendTyping = it
            }
            SwitchRow("Отчёты о прочтении", "собеседник видит, что вы открыли его сообщения", receipts) {
                receipts = it; p.sendReceipts = it
            }
            SwitchRow("Блокировка приложения", "отпечаток / лицо / PIN устройства при открытии", bio) {
                bio = it; p.biometricLock = it
            }
            Text("Тема", color = P.Muted, fontSize = 13.sp)
            ThemePicker(vm.themePref) { vm.themePref = it; p.theme = it }
            Text("Ключ узла хранится зашифрованным (identity.key.enc), ключ шифрования — в Android Keystore. " +
                "Локальный API узла закрыт токеном: другие приложения телефона не могут им воспользоваться.",
                color = P.Muted, fontSize = 12.sp)
        }

        // ── журнал ──
        SectionCard("Хранение") { StorageSection(vm) }

        SectionCard("Журнал узла") {
            var lines by remember { mutableStateOf(vm.logTail(80)) }
            LaunchedEffect(Unit) { while (true) { lines = vm.logTail(80); delay(2000) } }
            Box(
                Modifier.fillMaxWidth().heightIn(max = 320.dp).background(P.Input, RoundedCornerShape(10.dp)).padding(10.dp)
                    .verticalScroll(rememberScrollState(Int.MAX_VALUE)).horizontalScroll(rememberScrollState())
            ) {
                Text(lines.joinToString("\n").ifEmpty { "пусто" }, fontFamily = FontFamily.Monospace, fontSize = 10.sp, color = P.Muted)
            }
        }
    }
}

@Composable
private fun SwitchRow(title: String, sub: String?, value: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, color = if (enabled) P.Text else P.Muted)
            if (sub != null) Text(sub, color = P.Muted, fontSize = 12.sp)
        }
        Switch(value, onChange, enabled = enabled)
    }
}

@Composable
fun InviteDialog(inv: Invite, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var name by remember { mutableStateOf("Компьютер") }
    AlertDialog(
        onDismissRequest = onDismiss, containerColor = P.Side,
        title = { Text("Подключиться к узлу?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                KeyValue("Адрес", inv.addr)
                inv.nodeId?.let { KeyValue("NodeID", it.take(24) + "…", mono = true) }
                Text("Адрес станет bootstrap-пиром, узел будет перезапущен.", color = P.Muted, fontSize = 13.sp)
                if (inv.nodeId != null) OutlinedTextField(name, { name = it }, label = { Text("Имя контакта") }, singleLine = true)
            }
        },
        confirmButton = { TextButton(onClick = { onConfirm(name) }) { Text("Подключить") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Отмена") } },
    )
}


/** Шифрование истории и срок хранения; действия со старыми записями (последнее — красное). */
@Composable
private fun StorageSection(vm: AppViewModel) {
    val r = vm.retention
    val enc = r?.optBoolean("encrypted") == true
    Pill(if (enc) "история зашифрована · ключ в Android Keystore" else "история не зашифрована", if (enc) P.Green else P.Yellow)
    if (r != null) Text("Записей: ${r.optInt("records")} · срок хранения: ${r.optInt("retention_days")} дн. · старше срока: ${r.optInt("expired")}",
        color = P.Muted, fontSize = 13.sp)
    Text("Ключ узла и история на телефоне хранятся зашифрованными; принятые файлы — в закрытой папке приложения.",
        color = P.Muted, fontSize = 12.sp)
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        androidx.compose.material3.OutlinedButton(onClick = { vm.historyAction("reencrypt") }, enabled = enc) { Text("Перешифровать") }
        if ((r?.optInt("expired") ?: 0) > 0) androidx.compose.material3.OutlinedButton(onClick = { vm.retentionDialog = true }) { Text("Старые записи…") }
    }
}

/** Диалог: записи старше срока — удалить / перешифровать и оставить / вынести в открытый файл (красная). */
@Composable
fun RetentionDialog(vm: AppViewModel) {
    if (!vm.retentionDialog) return
    val r = vm.retention ?: return
    androidx.compose.material3.AlertDialog(
        onDismissRequest = { vm.dismissRetention() }, containerColor = P.Side,
        title = { Text("Старые записи истории") },
        text = {
            androidx.compose.foundation.layout.Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                val n = r.optInt("expired")
                val w = when { n % 10 == 1 && n % 100 != 11 -> "запись"; n % 10 in 2..4 && n % 100 !in 12..14 -> "записи"; else -> "записей" }
                Text("$n $w старше ${r.optInt("retention_days")} дней. Что с ними сделать?")
                androidx.compose.material3.Button(onClick = { vm.historyAction("delete") }, modifier = Modifier.fillMaxWidth()) { Text("Удалить") }
                androidx.compose.material3.OutlinedButton(onClick = { vm.historyAction("keep") }, enabled = r.optBoolean("encrypted"),
                    modifier = Modifier.fillMaxWidth()) { Text("Перешифровать и оставить ещё на год") }
                androidx.compose.material3.Button(onClick = { vm.historyAction("export") }, modifier = Modifier.fillMaxWidth(),
                    colors = androidx.compose.material3.ButtonDefaults.buttonColors(containerColor = P.Red, contentColor = androidx.compose.ui.graphics.Color.White)) {
                    Text("⚠ Вынести в открытый файл")
                }
                Text("Открытый файл НЕ зашифрован: переписку прочитает любой, к кому он попадёт. Сразу после вывода " +
                    "откроется «Поделиться» — перенесите файл в надёжное место; из истории записи удаляются.",
                    color = P.Red, fontSize = 12.sp)
            }
        },
        confirmButton = { androidx.compose.material3.TextButton(onClick = { vm.dismissRetention() }) { Text("Позже") } },
    )
}
