package org.p2poverlay.android.ui

import android.content.Context
import android.content.Intent
import android.webkit.MimeTypeMap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
        item {
            Text("ПЕРЕДАЧИ", style = androidx.compose.material3.MaterialTheme.typography.labelSmall, color = P.Muted,
                modifier = Modifier.padding(top = 4.dp))
        }
        if (vm.transfers.isEmpty()) item { Text("Пока пусто", color = P.Muted) }
        items(vm.transfers, key = { it.optString("transfer_id") }) { t -> TransferRow(vm, ctx, t) }
        item { if (vm.downloadsDir.isNotEmpty()) Text("Принятые файлы: ${vm.downloadsDir}", color = P.Muted, fontSize = 11.sp,
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
        else -> "ошибка" to P.Red
    }
    SectionCard {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.AutoMirrored.Filled.InsertDriveFile, null, tint = P.Accent)
            Column(Modifier.weight(1f)) {
                Text(t.optString("name"), maxLines = 1)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Icon(if (out) Icons.Filled.ArrowUpward else Icons.Filled.ArrowDownward, null, tint = P.Muted,
                        modifier = Modifier.padding(0.dp))
                    Text("${vm.nameOf(t.optString("peer"))} · ${humanBytes(t.optLong("size"))}", color = P.Muted, fontSize = 12.sp)
                }
            }
            Pill(label, color)
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

private fun openFile(ctx: Context, f: File, action: String) {
    runCatching {
        val uri = FileProvider.getUriForFile(ctx, ctx.packageName + ".files", f)
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(f.extension.lowercase()) ?: "*/*"
        val i = Intent(action).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        if (action == Intent.ACTION_SEND) i.setType(mime).putExtra(Intent.EXTRA_STREAM, uri) else i.setDataAndType(uri, mime)
        ctx.startActivity(Intent.createChooser(i, f.name))
    }
}
