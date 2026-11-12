package org.p2poverlay.android.ui

import android.content.Context
import android.content.Intent
import android.webkit.MimeTypeMap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.InsertDriveFile
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.FileUpload
import androidx.compose.material.icons.automirrored.filled.OpenInNew
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ExposedDropdownMenuAnchorType
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.FileProvider
import org.json.JSONObject
import org.p2poverlay.android.AppViewModel
import java.io.File

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FilesScreen(vm: AppViewModel, sharedUri: android.net.Uri?, onSharedConsumed: () -> Unit) {
    val ctx = LocalContext.current
    val peers = (vm.contacts.map { it.nodeId } + vm.chats().map { it.chat }.filter { !vm.isGroup(it) }).distinct()
    var target by rememberSaveable { mutableStateOf(peers.firstOrNull() ?: "") }
    var open by rememberSaveable { mutableStateOf(false) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null && target.isNotEmpty()) vm.sendFile(target, uri)
    }
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            SectionCard("Отправить файл", Modifier.padding(top = 12.dp)) {
                Text("Файл режется на блоки по 40 КиБ, каждый шифруется для получателя и идёт через туннель; " +
                    "получатель сверяет SHA-256.", color = P.Muted, fontSize = 13.sp)
                ExposedDropdownMenuBox(expanded = open, onExpandedChange = { open = it }) {
                    OutlinedTextField(
                        value = if (target.isEmpty()) "" else vm.nameOf(target), onValueChange = {}, readOnly = true,
                        label = { Text("Получатель") },
                        trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(open) },
                        modifier = Modifier.fillMaxWidth().menuAnchor(ExposedDropdownMenuAnchorType.PrimaryNotEditable),
                    )
                    ExposedDropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                        peers.forEach { p ->
                            DropdownMenuItem(text = { Text(vm.nameOf(p)) }, onClick = { target = p; open = false },
                                leadingIcon = { Avatar(p, vm.nameOf(p), 28.dp) })
                        }
                    }
                }
                val busy = vm.sendingFile != null
                Button(
                    enabled = target.isNotEmpty() && !busy,
                    onClick = {
                        if (sharedUri != null) { vm.sendFile(target, sharedUri); onSharedConsumed() }
                        else pick.launch(arrayOf("*/*"))
                    },
                ) {
                    Icon(Icons.Filled.FileUpload, null)
                    Text(when {
                        busy -> "  Отправка «${vm.sendingFile}»…"
                        sharedUri != null -> "  Отправить присланный файл"
                        else -> "  Выбрать и отправить"
                    })
                }
                if (peers.isEmpty()) Text("Нет контактов — добавьте компьютер через QR на вкладке «Узел».",
                    color = P.Yellow, fontSize = 13.sp)
            }
        }
        if (vm.downloads.isNotEmpty()) {
            item {
                Text("ПОЛУЧЕННЫЕ ФАЙЛЫ — ПО СОБЕСЕДНИКАМ", style = androidx.compose.material3.MaterialTheme.typography.labelSmall,
                    color = P.Muted, modifier = Modifier.padding(top = 4.dp))
            }
            vm.downloads.forEach { f ->
                val prefix = f.optString("peer_prefix")
                val files = f.optJSONArray("files")?.let { a -> (0 until a.length()).map { a.getJSONObject(it) } } ?: emptyList()
                item(key = "dl-$prefix") { DownloadFolder(vm, ctx, prefix, files) }
            }
        }
        item {
            Text("ПЕРЕДАЧИ", style = androidx.compose.material3.MaterialTheme.typography.labelSmall, color = P.Muted,
                modifier = Modifier.padding(top = 4.dp))
        }
        if (vm.transfers.isEmpty()) item { Text("Пока пусто", color = P.Muted) }
        // по собеседникам: у каждого своя папка в downloads/ (первые 16 символов NodeID)
        vm.transfers.groupBy { it.optString("peer") }.forEach { (peer, list) ->
            item(key = "h-$peer") {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.padding(top = 8.dp)) {
                    Avatar(peer, vm.nameOf(peer), 26.dp)
                    Text(vm.nameOf(peer), color = P.Text, fontSize = 14.sp)
                    Text("· папка ${peer.take(16)}/ · ${list.size}", color = P.Muted, fontSize = 12.sp)
                }
            }
            items(list, key = { it.optString("transfer_id") }) { t -> TransferRow(vm, ctx, t) }
        }
        item { if (vm.downloadsDir.isNotEmpty()) Text("Принятые файлы: ${vm.downloadsDir}/<папка собеседника>/ " +
            "(больше 2 ГиБ — в подпапке «Большие файлы»)", color = P.Muted, fontSize = 11.sp,
            modifier = Modifier.padding(bottom = 16.dp)) }
    }
}

@Composable
private fun TransferRow(vm: AppViewModel, ctx: Context, t: JSONObject) {
    val out = t.optString("dir") == "out"
    val status = t.optString("status")
    val blocks = t.optInt("blocks").coerceAtLeast(1)
    val done = t.optInt("done_blocks")
    val (label, color) = when (status) {
        "sent", "delivered" -> "доставлен · SHA-256 ✓" to P.Green
        "received" -> "получен · SHA-256 ✓" to P.Green
        "sending" -> "отправка" to P.Yellow
        "receiving" -> "приём" to P.Yellow
        "rejected" -> "отклонён" to P.Red
        "offered" -> "ждёт вашего решения" to P.Yellow
        "waiting" -> "ждёт согласия получателя" to P.Yellow
        else -> "ошибка" to P.Red
    }
    SectionCard {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.AutoMirrored.Filled.InsertDriveFile, null, tint = P.Accent)
            Column(Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(t.optString("name"), maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                    ExtBadge(t.optString("ext"))
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Icon(if (out) Icons.Filled.ArrowUpward else Icons.Filled.ArrowDownward, null, tint = P.Muted,
                        modifier = Modifier.padding(0.dp))
                    Text("${vm.nameOf(t.optString("peer"))} · ${humanBytes(t.optLong("size"))}", color = P.Muted, fontSize = 12.sp)
                }
            }
            Pill(label, color)
        }
        val warn = t.optString("warning").takeIf { it.isNotEmpty() && it != "null" }
        if (status == "offered") {
            Text("Нужно ваше согласие — без него ничего не пишется на диск." + (warn?.let { " Почему: $it." } ?: ""),
                color = if (warn?.startsWith("⚠") == true) P.Yellow else P.Muted, fontSize = 12.sp)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { vm.fileDecision(t.optString("transfer_id"), true) }) { Text("Принять") }
                androidx.compose.material3.OutlinedButton(onClick = { vm.fileDecision(t.optString("transfer_id"), false) }) {
                    Text("Отклонить", color = P.Red)
                }
            }
        }
        if (status == "sending" || status == "receiving")
            LinearProgressIndicator(progress = { done.toFloat() / blocks }, modifier = Modifier.fillMaxWidth())
        t.optString("error").takeIf { it.isNotEmpty() && it != "null" }?.let { Text(it, color = P.Red, fontSize = 12.sp) }
        val path = t.optString("path")
        val local = if (path.isNotEmpty()) File(path) else null
        if (local != null && local.exists() && isImage(local.name)) Thumbnail(local, 180.dp) { openFile(ctx, local, Intent.ACTION_VIEW) }
        if (!out && status == "received" && path.isNotEmpty()) {
            Row {
                IconButton(onClick = { openFile(ctx, File(path), Intent.ACTION_VIEW) }) { Icon(Icons.AutoMirrored.Filled.OpenInNew, "Открыть") }
                IconButton(onClick = { openFile(ctx, File(path), Intent.ACTION_SEND) }) { Icon(Icons.Filled.Share, "Поделиться") }
            }
        }
    }
}

/** Папка собеседника: его файлы, открыть или переслать; очень большие помечены. */
@Composable
private fun DownloadFolder(vm: AppViewModel, ctx: Context, prefix: String, files: List<JSONObject>) {
    var all by rememberSaveable(prefix) { mutableStateOf(false) }
    val owner = vm.folderOwner(prefix)
    SectionCard {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (prefix.isNotEmpty()) Avatar(vm.folderOwnerId(prefix) ?: prefix, owner ?: prefix, 32.dp)
            Column(Modifier.weight(1f)) {
                Text(owner ?: if (prefix.isEmpty()) "Принятые раньше (общая папка)" else "Неизвестный узел", fontSize = 15.sp)
                Text(if (prefix.isEmpty()) "downloads/" else "downloads/$prefix/ · ${files.size} ${plural(files.size)}",
                    color = P.Muted, fontSize = 12.sp)
            }
        }
        (if (all) files else files.take(4)).forEach { f ->
            val file = File(f.optString("path"))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(f.optString("name"), maxLines = 1, fontSize = 14.sp, modifier = Modifier.weight(1f, fill = false))
                        ExtBadge(f.optString("ext"))
                    }
                    f.optString("name_warning").takeIf { it.isNotEmpty() && it != "null" }?.let { Text(it, color = P.Yellow, fontSize = 11.sp) }
                    Text(humanBytes(f.optLong("size")) + if (f.optBoolean("large")) " · «Большие файлы»" else "",
                        color = if (f.optBoolean("large")) P.Yellow else P.Muted, fontSize = 12.sp)
                }
                IconButton(onClick = { openFile(ctx, file, Intent.ACTION_VIEW) }) { Icon(Icons.AutoMirrored.Filled.OpenInNew, "Открыть") }
                IconButton(onClick = { openFile(ctx, file, Intent.ACTION_SEND) }) { Icon(Icons.Filled.Share, "Поделиться") }
            }
        }
        if (files.size > 4) androidx.compose.material3.TextButton(onClick = { all = !all }) {
            Text(if (all) "Свернуть" else "Ещё ${files.size - 4}")
        }
    }
}

/** Настоящее расширение отдельной меткой: имя со скрытыми символами может «показывать» другое. */
@Composable
private fun ExtBadge(ext: String) {
    if (ext.isEmpty() || ext == "null") return
    Text(ext.uppercase(), fontSize = 11.sp, color = P.Muted,
        modifier = Modifier.border(1.dp, P.Muted.copy(alpha = 0.5f), androidx.compose.foundation.shape.RoundedCornerShape(4.dp))
            .padding(horizontal = 4.dp))
}

private fun plural(n: Int) = when { n % 10 == 1 && n % 100 != 11 -> "файл"; n % 10 in 2..4 && n % 100 !in 12..14 -> "файла"; else -> "файлов" }

private fun openFile(ctx: Context, f: File, action: String) {
    runCatching {
        val uri = FileProvider.getUriForFile(ctx, ctx.packageName + ".files", f)
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(f.extension.lowercase()) ?: "*/*"
        val i = Intent(action).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        if (action == Intent.ACTION_SEND) i.setType(mime).putExtra(Intent.EXTRA_STREAM, uri) else i.setDataAndType(uri, mime)
        ctx.startActivity(Intent.createChooser(i, f.name))
    }
}
