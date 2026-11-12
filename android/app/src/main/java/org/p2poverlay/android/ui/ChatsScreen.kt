package org.p2poverlay.android.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Done
import androidx.compose.material.icons.filled.DoneAll
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.ErrorOutline
import androidx.compose.material.icons.filled.Groups
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PhotoCamera
import androidx.compose.material.icons.filled.Route
import androidx.compose.material.icons.filled.Schedule
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Inventory2
import androidx.compose.material.icons.filled.VerifiedUser
import androidx.compose.material.icons.outlined.Forum
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import kotlinx.coroutines.launch
import org.json.JSONObject
import org.p2poverlay.android.AppViewModel
import org.p2poverlay.android.ChatMsg
import org.p2poverlay.android.RunState

// ── список бесед ─────────────────────────────────────────────────────────────

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ChatsList(vm: AppViewModel, onOpen: (String) -> Unit, onScan: () -> Unit) {
    var newChat by remember { mutableStateOf(false) }
    var newGroup by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf<String?>(null) }
    val chats = vm.chats()
    Scaffold(
        containerColor = P.Screen,
        floatingActionButton = {
            Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                SmallFloatingActionButton(onClick = { newGroup = true }, containerColor = P.Card) {
                    Icon(Icons.Filled.Groups, "Новая группа", tint = P.Accent)
                }
                ExtendedFloatingActionButton(
                    onClick = { newChat = true },
                    icon = { Icon(Icons.Filled.Edit, null) },
                    text = { Text("Новый чат") },
                    containerColor = P.AccentDim, contentColor = Color.White,
                )
            }
        },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize()) {
            TextField(
                vm.search, { vm.search = it }, singleLine = true,
                placeholder = { Text("Поиск по чатам и сообщениям") },
                leadingIcon = { Icon(Icons.Filled.Search, null, tint = P.Muted) },
                trailingIcon = { if (vm.search.isNotEmpty()) IconButton(onClick = { vm.search = "" }) { Icon(Icons.Filled.Close, "Очистить") } },
                shape = RoundedCornerShape(24.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = P.Input, unfocusedContainerColor = P.Input,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                ),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
            )
            if (chats.isEmpty()) {
                EmptyState(Modifier, vm.runState.value is RunState.Running, vm.search.isNotEmpty())
            } else LazyColumn(Modifier.fillMaxSize()) {
                items(chats, key = { it.chat }) { c ->
                    Row(
                        Modifier.fillMaxWidth()
                            .combinedClickable(onClick = { onOpen(c.chat) }, onLongClick = { deleting = c.chat })
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(14.dp),
                    ) {
                        val name = vm.nameOf(c.chat)
                        Box {
                            Avatar(c.chat, name, 50.dp)
                            if (vm.isGroup(c.chat)) Icon(Icons.Filled.Groups, null, tint = Color.White,
                                modifier = Modifier.align(Alignment.BottomEnd).size(18.dp).background(P.AccentDim, CircleShape).padding(2.dp))
                        }
                        Column(Modifier.weight(1f)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(name, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, maxLines = 1,
                                    overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                if (c.chat in vm.verified) Icon(Icons.Filled.VerifiedUser, "сверен", tint = P.Green, modifier = Modifier.size(16.dp))
                                if (c.last.ts > 0) Text(" " + whenShort(c.last.ts), color = P.Muted, fontSize = 12.sp)
                            }
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                val typing = vm.typing.containsKey(c.chat)
                                val preview = when {
                                    typing -> "печатает…"
                                    c.last.out && c.last.ts > 0 -> "Вы: " + c.last.text
                                    vm.isGroup(c.chat) && c.last.ts > 0 -> "${vm.nameOf(c.last.peer)}: ${c.last.text}"
                                    else -> c.last.text
                                }
                                Text(preview, color = if (typing) P.Accent else P.Muted, fontSize = 14.sp,
                                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                if (c.unread > 0) Box(
                                    Modifier.size(22.dp).background(P.Accent, CircleShape),
                                    contentAlignment = Alignment.Center,
                                ) { Text("${c.unread}", color = Color.White, fontSize = 12.sp) }
                            }
                        }
                    }
                    HorizontalDivider(Modifier.padding(start = 80.dp), color = P.Divider)
                }
            }
        }
    }
    if (newChat) NewChatDialog(vm, onDismiss = { newChat = false }, onOpen = { newChat = false; onOpen(it) }, onScan = onScan)
    if (newGroup) NewGroupDialog(vm, onDismiss = { newGroup = false }) { newGroup = false; onOpen(it) }
    deleting?.let { chat ->
        AlertDialog(
            onDismissRequest = { deleting = null }, containerColor = P.Side,
            title = { Text("Удалить переписку?") },
            text = { Text("«${vm.nameOf(chat)}»: история на этом устройстве будет удалена. У собеседника ничего не изменится.") },
            confirmButton = { TextButton(onClick = { vm.deleteChat(chat); deleting = null }) { Text("Удалить", color = P.Red) } },
            dismissButton = { TextButton(onClick = { deleting = null }) { Text("Отмена") } },
        )
    }
}

@Composable
private fun EmptyState(modifier: Modifier, running: Boolean, searching: Boolean) {
    Column(
        modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(Icons.Outlined.Forum, null, tint = P.Muted, modifier = Modifier.size(64.dp))
        Spacer(Modifier.size(12.dp))
        Text(if (searching) "Ничего не найдено" else "Пока нет переписок", style = MaterialTheme.typography.titleMedium)
        Spacer(Modifier.size(6.dp))
        if (!searching) Text(
            if (running) "Нажмите «Новый чат» и выберите контакт, @псевдоним или NodeID. " +
                "Сообщения идут через туннель из ≥ 2 ретрансляторов и шифруются сквозным ключом получателя."
            else "Узел не запущен. Откройте вкладку «Узел», чтобы подключиться к компьютеру.",
            color = P.Muted, fontSize = 14.sp,
        )
    }
}

@Composable
fun NewChatDialog(vm: AppViewModel, onDismiss: () -> Unit, onOpen: (String) -> Unit, onScan: () -> Unit) {
    var input by rememberSaveable { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = P.Side,
        title = { Text("Новый чат") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedTextField(input, { input = it }, label = { Text("@псевдоним или NodeID") }, singleLine = true,
                    modifier = Modifier.fillMaxWidth())
                if (vm.contacts.isNotEmpty()) {
                    Text("КОНТАКТЫ", style = MaterialTheme.typography.labelSmall, color = P.Muted)
                    vm.contacts.forEach { c ->
                        Row(
                            Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable { onOpen(c.nodeId) }.padding(6.dp),
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Avatar(c.nodeId, c.name, 34.dp)
                            Column {
                                Text(c.name)
                                Text("${c.nodeId.take(16)}…", color = P.Muted, fontSize = 12.sp)
                            }
                        }
                    }
                }
                TextButton(onClick = { onDismiss(); onScan() }) { Text("Сканировать QR компьютера") }
            }
        },
        confirmButton = {
            TextButton(enabled = !busy && input.isNotBlank(), onClick = {
                busy = true
                scope.launch { vm.resolve(input)?.let(onOpen); busy = false }
            }) { Text(if (busy) "Поиск в DHT…" else "Открыть") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Отмена") } },
    )
}

@Composable
fun NewGroupDialog(vm: AppViewModel, onDismiss: () -> Unit, onCreated: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    var picked by remember { mutableStateOf(setOf<String>()) }
    AlertDialog(
        onDismissRequest = onDismiss, containerColor = P.Side,
        title = { Text("Новая группа") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                OutlinedTextField(name, { name = it }, label = { Text("Название") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                Text("Каждый участник получает свою копию, зашифрованную его ключом; состав группы едет в сообщении.",
                    color = P.Muted, fontSize = 12.sp)
                if (vm.contacts.isEmpty()) Text("Сначала добавьте контакты.", color = P.Yellow)
                vm.contacts.forEach { c ->
                    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()
                        .clickable { picked = if (c.nodeId in picked) picked - c.nodeId else picked + c.nodeId }) {
                        Checkbox(c.nodeId in picked, { picked = if (it) picked + c.nodeId else picked - c.nodeId })
                        Avatar(c.nodeId, c.name, 30.dp)
                        Text("  " + c.name)
                    }
                }
            }
        },
        confirmButton = {
            TextButton(enabled = name.isNotBlank() && picked.isNotEmpty() && vm.myId.isNotEmpty(),
                onClick = { onCreated(vm.createGroup(name, picked.toList())) }) { Text("Создать") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Отмена") } },
    )
}

// ── беседа ───────────────────────────────────────────────────────────────────

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Conversation(vm: AppViewModel, chat: String, onBack: () -> Unit) {
    BackHandler(onBack = onBack)
    val ctx = LocalContext.current
    val msgs = vm.conversation(chat)
    val group = vm.isGroup(chat)
    var text by rememberSaveable(chat) { mutableStateOf("") }
    var menu by remember { mutableStateOf(false) }
    var dialog by remember { mutableStateOf<String?>(null) }
    var photo by remember { mutableStateOf<java.io.File?>(null) }
    val list = rememberLazyListState()
    val pickFile = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) vm.sendFiles(chat, uris)
    }
    val takePhoto = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { ok ->
        val f = photo
        if (ok && f != null && f.length() > 0) vm.sendFile(chat, Uri.fromFile(f)) else f?.delete()
    }
    fun launchCamera() {
        val f = vm.newPhotoFile().also { photo = it }
        takePhoto.launch(FileProvider.getUriForFile(ctx, ctx.packageName + ".files", f))
    }
    val camPerm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { if (it) launchCamera() }
    LaunchedEffect(msgs.size) {
        if (msgs.isNotEmpty()) list.animateScrollToItem(msgs.size - 1)
        vm.markSeen(chat)
    }
    val name = vm.nameOf(chat)
    val typing = vm.typing.containsKey(chat)
    val callWithMic = rememberWithMic { vm.startCall(it) }
    Scaffold(
        containerColor = P.Main,
        topBar = {
            Column {
            TopAppBar(
                colors = TopAppBarDefaults.topAppBarColors(containerColor = P.Side),
                navigationIcon = { IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Назад") } },
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                        modifier = Modifier.clickable { dialog = if (group) "members" else "rename" }) {
                        Avatar(chat, name, 38.dp)
                        Column {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(name, fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                if (chat in vm.verified) Icon(Icons.Filled.VerifiedUser, "сверен", tint = P.Green,
                                    modifier = Modifier.padding(start = 4.dp).size(16.dp))
                            }
                            val sub = when {
                                typing -> "печатает…"
                                group -> "${vm.groups()[chat.removePrefix("g:")]?.members?.size ?: 0} участников · E2E каждому"
                                else -> "${chat.take(16)}… · E2E"
                            }
                            Text(sub, color = if (typing) P.Accent else P.Muted, fontSize = 12.sp)
                        }
                    }
                },
                actions = {
                    IconButton(onClick = { callWithMic(chat) }, enabled = !vm.callLive) {
                        Icon(Icons.Filled.Call, if (group) "Групповой звонок" else "Позвонить", tint = if (vm.callLive) P.Muted else P.Accent)
                    }
                    IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, "Меню") }
                    DropdownMenu(menu, { menu = false }) {
                        if (!group) {
                            DropdownMenuItem(text = { Text("Маршрут туннеля") }, leadingIcon = { Icon(Icons.Filled.Route, null) },
                                onClick = { menu = false; dialog = "route" })
                            DropdownMenuItem(text = { Text("Номер безопасности") }, leadingIcon = { Icon(Icons.Filled.VerifiedUser, null) },
                                onClick = { menu = false; dialog = "safety" })
                            DropdownMenuItem(text = { Text("Переименовать контакт") }, leadingIcon = { Icon(Icons.Filled.Edit, null) },
                                onClick = { menu = false; dialog = "rename" })
                        } else DropdownMenuItem(text = { Text("Участники") }, leadingIcon = { Icon(Icons.Filled.Groups, null) },
                            onClick = { menu = false; dialog = "members" })
                        DropdownMenuItem(text = { Text("Проверить почтовые ящики DHT") }, leadingIcon = { Icon(Icons.Filled.Inventory2, null) },
                            onClick = { menu = false; vm.mailboxPoll() })
                        DropdownMenuItem(text = { Text("Удалить переписку", color = P.Red) }, leadingIcon = { Icon(Icons.Filled.Delete, null, tint = P.Red) },
                            onClick = { menu = false; dialog = "delete" })
                    }
                },
            )
            Decor.TopEdge()
            }
        },
        bottomBar = {
            Surface(color = P.Side) {
                Column(Modifier.navigationBarsPadding().imePadding()) {
                    vm.sendingFile?.let {
                        Text("Отправка: $it через туннель…", color = P.Yellow, fontSize = 12.sp,
                            modifier = Modifier.padding(start = 16.dp, top = 6.dp))
                    }
                    Row(Modifier.padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (!group) {
                            IconButton(onClick = { pickFile.launch(arrayOf("*/*")) }) {
                                Icon(Icons.Filled.AttachFile, "Файл", tint = P.Muted)
                            }
                            IconButton(onClick = {
                                if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) launchCamera()
                                else camPerm.launch(Manifest.permission.CAMERA)
                            }) { Icon(Icons.Filled.PhotoCamera, "Фото", tint = P.Muted) }
                        }
                        TextField(
                            text, { text = it; if (it.isNotBlank()) vm.onTyping(chat) }, placeholder = { Text("Сообщение") },
                            modifier = Modifier.weight(1f), maxLines = 5,
                            shape = RoundedCornerShape(22.dp),
                            colors = TextFieldDefaults.colors(
                                focusedContainerColor = P.Input, unfocusedContainerColor = P.Input,
                                focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                            ),
                        )
                        IconButton(enabled = text.isNotBlank(), onClick = { vm.send(chat, text); text = "" }) {
                            Icon(Icons.AutoMirrored.Filled.Send, "Отправить", tint = if (text.isNotBlank()) P.Accent else P.Muted)
                        }
                    }
                }
            }
        },
    ) { pad ->
        Box(Modifier.padding(pad).fillMaxSize()) { Decor.Backdrop(Modifier.matchParentSize()) }
        LazyColumn(
            Modifier.padding(pad).fillMaxSize().padding(horizontal = 10.dp), state = list,
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            item { Spacer(Modifier.size(8.dp)) }
            items(msgs, key = { it.key }) { m -> Bubble(vm, m, group) }
            if (typing) item { Text("печатает…", color = P.Accent, fontSize = 13.sp, modifier = Modifier.padding(6.dp)) }
            item { Spacer(Modifier.size(8.dp)) }
        }
    }
    when (dialog) {
        "rename" -> RenameDialog(vm, chat, name) { dialog = null }
        "safety" -> SafetyDialog(vm, chat) { dialog = null }
        "route" -> RouteDialog(vm, chat) { dialog = null }
        "members" -> MembersDialog(vm, chat) { dialog = null }
        "delete" -> AlertDialog(
            onDismissRequest = { dialog = null }, containerColor = P.Side,
            title = { Text("Удалить переписку?") },
            text = { Text("История на этом устройстве будет удалена.") },
            confirmButton = { TextButton(onClick = { vm.deleteChat(chat); dialog = null; onBack() }) { Text("Удалить", color = P.Red) } },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text("Отмена") } },
        )
    }
}

@Composable
private fun Bubble(vm: AppViewModel, m: ChatMsg, group: Boolean) {
    val ctx = LocalContext.current
    val failed = m.status == "failed"
    val queued = m.status == "queued"
    val fg = if (m.out) Color.White else P.Text
    Row(Modifier.fillMaxWidth(), horizontalArrangement = if (m.out) Arrangement.End else Arrangement.Start) {
        if (group && !m.out) { Avatar(m.peer, vm.nameOf(m.peer), 28.dp); Spacer(Modifier.size(6.dp)) }
        Surface(
            color = when { failed -> P.Red.copy(alpha = 0.25f); m.out -> P.AccentDim; else -> P.BubbleIn },
            shape = RoundedCornerShape(18.dp, 18.dp, if (m.out) 4.dp else 18.dp, if (m.out) 18.dp else 4.dp),
            shadowElevation = if (P.dark || m.out) 0.dp else 1.dp,
            modifier = with(Decor) { Modifier.bubbleDecor(m.out && !failed, m.key) }.widthIn(max = 300.dp),
        ) {
            Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp)) {
                if (group && !m.out) Text(vm.nameOf(m.peer), color = colorFor(m.peer), fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                val file = vm.localFile(m)
                if (file != null && isImage(file.name)) {
                    Thumbnail(file) { openLocal(ctx, file) }
                    Spacer(Modifier.size(4.dp))
                    Text(file.name, fontSize = 12.sp, color = fg.copy(alpha = 0.75f))
                } else Text(m.text, fontSize = 15.sp,
                    color = if (m.status == "call_missed" || m.status == "call_rejected") (if (m.out) Color.White else P.Red) else fg)
                Row(Modifier.align(Alignment.End), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    val meta = buildString {
                        if (m.out && m.rttMs != null && !group) append("${m.rttMs} мс · ")
                        if (queued) append("в почтовом ящике · ")
                        if (m.readMs != null) append("прочитано · ")
                        append(hhmm(m.ts))
                    }
                    Text(meta, fontSize = 11.sp, color = fg.copy(alpha = 0.65f))
                    if (m.out && !m.status.startsWith("call_")) {
                        val (icon, tint) = when {
                            m.readMs != null -> Icons.Filled.DoneAll to P.Read
                            m.status == "delivered" -> Icons.Filled.DoneAll to Color.White
                            m.status == "sending" || queued -> Icons.Filled.Schedule to Color.White.copy(alpha = 0.6f)
                            m.status == "failed" || m.status == "partial" -> Icons.Filled.ErrorOutline to P.Red
                            else -> Icons.Filled.Done to Color.White.copy(alpha = 0.7f)
                        }
                        Icon(icon, m.status, tint = tint, modifier = Modifier.size(14.dp))
                    }
                }
                if ((failed || m.status == "partial") && m.error != null) Text(m.error, fontSize = 11.sp, color = P.Red)
            }
        }
    }
}

private fun openLocal(ctx: android.content.Context, f: java.io.File) {
    runCatching {
        val uri = FileProvider.getUriForFile(ctx, ctx.packageName + ".files", f)
        val mime = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(f.extension.lowercase()) ?: "*/*"
        ctx.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
    }
}

// ── диалоги беседы ───────────────────────────────────────────────────────────

@Composable
private fun RenameDialog(vm: AppViewModel, peer: String, current: String, onDone: () -> Unit) {
    var name by remember { mutableStateOf(if (current.endsWith("…")) "" else current) }
    AlertDialog(
        onDismissRequest = onDone, containerColor = P.Side,
        title = { Text("Контакт") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(peer, fontSize = 12.sp, color = P.Muted, fontFamily = FontFamily.Monospace)
                OutlinedTextField(name, { name = it }, label = { Text("Имя") }, singleLine = true)
            }
        },
        confirmButton = { TextButton(enabled = name.isNotBlank(), onClick = { vm.saveContact(name, peer); onDone() }) { Text("Сохранить") } },
        dismissButton = {
            TextButton(onClick = { vm.removeContact(peer); onDone() }) { Text("Удалить из контактов", color = P.Red) }
        },
    )
}

/**
 * Номер безопасности: одинаков у обоих собеседников, зависит только от пары NodeID (а NodeID —
 * хэш открытого ключа). Совпал при личной сверке или по телефону → ключ собеседника не подменён.
 */
@Composable
private fun SafetyDialog(vm: AppViewModel, peer: String, onDone: () -> Unit) {
    var safety by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(peer) { safety = vm.safetyNumber(peer) }
    val ok = peer in vm.verified
    AlertDialog(
        onDismissRequest = onDone, containerColor = P.Side,
        title = { Text("Номер безопасности") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Сверьте цифры с ${vm.nameOf(peer)} лично или по телефону — у него на экране должны быть те же.",
                    color = P.Muted, fontSize = 13.sp)
                safety?.let {
                    SafetyGrid(it)
                    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { QrImage("p2p-safety:$it", 160.dp) }
                } ?: Text("…")
                if (ok) Pill("сверен", P.Green)
            }
        },
        confirmButton = {
            TextButton(onClick = { vm.setVerified(peer, !ok); onDone() }) { Text(if (ok) "Снять отметку" else "Совпадает — отметить") }
        },
        dismissButton = { TextButton(onClick = onDone) { Text("Закрыть") } },
    )
}

@Composable
private fun RouteDialog(vm: AppViewModel, peer: String, onDone: () -> Unit) {
    var r by remember { mutableStateOf<JSONObject?>(null) }
    var loaded by remember { mutableStateOf(false) }
    LaunchedEffect(peer) { r = vm.route(peer); loaded = true }
    AlertDialog(
        onDismissRequest = onDone, containerColor = P.Side,
        title = { Text("Маршрут туннеля") },
        text = {
            Column(Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                val t = r?.optJSONObject("tunnel")
                val hops = t?.optJSONArray("hops")
                when {
                    !loaded -> Text("…")
                    t == null || hops == null || hops.length() == 0 -> Text("Туннеля к собеседнику сейчас нет — он строится при отправке.", color = P.Muted)
                    else -> {
                        Text("Состояние: ${t.optString("state")} · туннелей в пуле: ${t.optJSONArray("pool")?.length() ?: 1} · выбор ${r?.optJSONObject("tunnel")?.optString("relay_selection")}",
                            color = P.Muted, fontSize = 13.sp)
                        val active = t.optString("state") == "Active"
                        Hop("Вы", "", vm.myId, true, first = true)
                        for (i in 0 until hops.length()) {
                            val h = hops.getJSONObject(i)
                            val id = h.optString("node_id")
                            // живость hop'а подтверждается сквозным TUNNEL_ACK; активный туннель — значит, цепочка цела
                            val ok = h.optBoolean("alive") || (active && h.optBoolean("confirmed", true))
                            Hop("Ретранслятор ${i + 1}", h.optString("addr"), id, ok)
                        }
                        Hop(vm.nameOf(peer), "получатель", peer, true)
                        Text("Ретрансляторы видят только соседей по цепочке и зашифрованный конверт.", color = P.Muted, fontSize = 12.sp)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDone) { Text("Закрыть") } },
    )
}

@Composable
private fun Hop(title: String, sub: String, id: String, alive: Boolean, first: Boolean = false) {
    if (!first) Text("      ↓", color = P.Muted)
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Avatar(id.ifEmpty { "me" }, title, 34.dp)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                if (!alive) Pill("не отвечает", P.Red)
            }
            if (sub.isNotEmpty()) Text(sub, fontSize = 12.sp, color = P.Muted)
            if (id.isNotEmpty()) Text("${id.take(16)}…", fontSize = 11.sp, color = P.Muted, fontFamily = FontFamily.Monospace)
        }
    }
}

@Composable
private fun MembersDialog(vm: AppViewModel, chat: String, onDone: () -> Unit) {
    val g = vm.groups()[chat.removePrefix("g:")]
    AlertDialog(
        onDismissRequest = onDone, containerColor = P.Side,
        title = { Text(g?.name ?: "Группа") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                g?.members?.forEach { m ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Avatar(m, vm.nameOf(m), 30.dp)
                        Text(vm.nameOf(m))
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDone) { Text("Закрыть") } },
    )
}
