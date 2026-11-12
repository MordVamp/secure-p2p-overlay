package org.p2poverlay.android

import android.app.Application
import android.net.Uri
import android.provider.OpenableColumns
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.SecureRandom

data class ChatMsg(
    val key: String,
    /** Беседа: NodeID собеседника или `g:<id>` группы. */
    val chat: String,
    /** Автор (для групп — кто написал; для исходящих — собеседник/группа). */
    val peer: String,
    val out: Boolean,
    val text: String,
    val ts: Long,
    /** delivered | sent | queued | partial | failed | received | sending */
    val status: String,
    val messageId: String = "",
    val rttMs: Long? = null,
    val error: String? = null,
    val readMs: Long? = null,
    val group: String? = null,
    val groupName: String? = null,
    val members: List<String> = emptyList(),
) {
    /** Вложение «📎 имя». */
    val fileName: String? get() = text.removePrefix("📎 ").takeIf { text.startsWith("📎 ") }
}

data class ChatSummary(val chat: String, val last: ChatMsg, val unread: Int)

data class Group(val id: String, val name: String, val members: List<String>)

class AppViewModel(app: Application) : AndroidViewModel(app) {
    val prefs = Prefs(app)
    val api = NodeApi.of(app)
    val runState = NodeService.state

    var status by mutableStateOf<JSONObject?>(null); private set
    var messages by mutableStateOf<List<ChatMsg>>(emptyList()); private set
    var transfers by mutableStateOf<List<JSONObject>>(emptyList()); private set
    var downloadsDir by mutableStateOf(""); private set
    var relays by mutableStateOf(JSONArray()); private set
    var contacts by mutableStateOf(prefs.contacts); private set
    var myAlias by mutableStateOf(prefs.myAlias); private set
    var verified by mutableStateOf(prefs.verified); private set
    var themePref by mutableStateOf(prefs.theme)
    var search by mutableStateOf("")
    val pending = mutableStateListOf<ChatMsg>()
    /** Кто печатает: NodeID → до какого момента. */
    val typing = mutableStateMapOf<String, Long>()
    private val seen = mutableStateOf(loadSeen())
    private val reported = HashSet<String>()
    private val lastTypingSent = HashMap<String, Long>()

    private val _toasts = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val toasts: SharedFlow<String> = _toasts
    fun toast(s: String) { _toasts.tryEmit(s) }

    val sentDir: File get() = File(getApplication<Application>().filesDir, "sent").apply { mkdirs() }

    /** Текущий сеанс звонка (JSON из /api/calls → sessions); после завершения виден ещё 2,5 с. */
    var call by mutableStateOf<JSONObject?>(null); private set
    var callMinimized by mutableStateOf(false)
    var muted by mutableStateOf(false); private set
    var speaker by mutableStateOf(false); private set
    private var callEndedAt = 0L

    init {
        if (prefs.autostart && runState.value is RunState.Stopped) NodeService.start(app)
        // сводка сети — редкий опрос (интервал по режиму)
        viewModelScope.launch {
            var tick = 0
            var wasRunning = false
            while (true) {
                val running = runState.value is RunState.Running
                if (running) {
                    refreshStatus()
                    if (!wasRunning) { refreshHistory(); refreshFiles(); watchContacts() }
                    if (tick % 3 == 0) refreshRelays()
                } else status = null
                wasRunning = running
                tick++
                delay(prefs.mode.statusPollMs)
            }
        }
        // история, файлы, «печатает», прочтение — push через /api/events
        viewModelScope.launch {
            var since = 0L
            while (true) {
                if (runState.value !is RunState.Running) { delay(1000); since = 0; continue }
                if (since == 0L) {
                    since = runCatching { api.get("/api/status").optLong("events_seq") }.getOrDefault(0L)
                    refreshHistory(); refreshFiles()
                }
                val r = runCatching {
                    api.get("/api/events", mapOf("since" to since.toString(), "timeout" to "25"), timeoutMs = 35_000)
                }.getOrElse { delay(2000); since = 0; continue }
                since = r.optLong("seq", since)
                val ev = r.optJSONArray("events") ?: continue
                var hist = false
                var files = false
                for (i in 0 until ev.length()) {
                    val e = ev.getJSONObject(i)
                    when (e.optString("kind")) {
                        "typing" -> typing[e.optString("peer")] = System.currentTimeMillis() + 6000
                        "message" -> { typing.remove(e.optString("peer")); hist = true }
                        "status", "read", "history" -> hist = true
                        "file" -> { files = true; hist = true }
                        "call" -> { refreshCalls(); hist = true }
                    }
                }
                if (hist) refreshHistory()
                if (files) refreshFiles()
            }
        }
        // идёт звонок — состояние, длительность, громкость участников и качество (каждые 0,5 с)
        viewModelScope.launch {
            while (true) {
                if (call != null && runState.value is RunState.Running) refreshCalls()
                delay(if (call != null) 500 else 2000)
            }
        }
        // «печатает» гаснет само
        viewModelScope.launch {
            while (true) {
                delay(1000)
                val now = System.currentTimeMillis()
                typing.entries.filter { it.value < now }.map { it.key }.forEach { typing.remove(it) }
            }
        }
    }

    private suspend fun refreshStatus() {
        runCatching { status = api.get("/api/status") }.onFailure { status = null }
    }

    private suspend fun refreshHistory() {
        runCatching {
            val e = api.get("/api/history", mapOf("limit" to "3000")).optJSONArray("entries") ?: JSONArray()
            messages = (0 until e.length()).map { e.getJSONObject(it) }.map { o ->
                val group = o.optStringOrNull("group")
                val peer = o.optString("peer")
                ChatMsg(
                    key = o.optString("message_id") + o.optLong("ts_ms") + peer,
                    chat = if (group != null) "g:$group" else peer,
                    peer = peer,
                    out = o.optString("dir") == "out",
                    text = o.optString("text", ""),
                    ts = o.optLong("ts_ms"),
                    status = o.optString("status"),
                    messageId = o.optString("message_id"),
                    rttMs = o.optLongOrNull("rtt_ms"),
                    error = o.optStringOrNull("error"),
                    readMs = o.optLongOrNull("read_ms"),
                    group = group,
                    groupName = o.optStringOrNull("group_name"),
                    members = o.optJSONArray("members")?.let { a -> (0 until a.length()).map { a.getString(it) } } ?: emptyList(),
                )
            }.sortedBy { it.ts }
        }
    }

    private suspend fun refreshFiles() {
        runCatching {
            val f = api.get("/api/files")
            downloadsDir = f.optString("downloads_dir")
            val t = f.optJSONArray("transfers") ?: JSONArray()
            transfers = (0 until t.length()).map { t.getJSONObject(it) }.sortedByDescending { it.optLong("started_ms") }
        }
    }

    private suspend fun refreshRelays() {
        runCatching { relays = api.get("/api/relays").optJSONArray("profiles") ?: JSONArray() }
    }

    /** Офлайн-доставка: узел проверяет почтовые ящики DHT и этих собеседников (ещё без переписки). */
    private suspend fun watchContacts() {
        val ids = contacts.map { it.nodeId }
        if (ids.isNotEmpty()) runCatching { api.post("/api/watch", mapOf("peers" to ids.joinToString(","))) }
    }

    // ── узел ─────────────────────────────────────────────────────────────

    fun startNode() = NodeService.start(getApplication())
    fun stopNode() = NodeService.stop(getApplication())
    fun restartNode() {
        viewModelScope.launch {
            stopNode()
            delay(1200)
            startNode()
        }
    }

    val myId: String get() = status?.optString("node_id").orEmpty()
    val myLink: String get() = status?.optString("link").orEmpty()

    /** Ссылка-приглашение с компьютера: bootstrap + контакт, затем перезапуск узла. */
    fun applyInvite(inv: Invite, name: String) {
        val peers = prefs.bootstrap.split(',').map { it.trim() }.filter { it.isNotEmpty() && it != inv.addr }
        prefs.bootstrap = (listOf(inv.addr) + peers).joinToString(", ")
        inv.nodeId?.let { saveContact(name.ifBlank { "Компьютер" }, it) }
        toast("Подключение к ${inv.addr}…")
        restartNode()
    }

    fun mailboxPoll() {
        viewModelScope.launch {
            runCatching { api.post("/api/mailbox/poll", timeoutMs = 120_000) }
                .onSuccess { toast("Почтовые ящики: новых сообщений ${it.optInt("received")}") }
                .onFailure { toast("Ошибка: ${it.message}") }
        }
    }

    // ── контакты, группы, чаты ───────────────────────────────────────────

    fun saveContact(name: String, id: String) {
        prefs.upsertContact(Contact(name.trim(), id.lowercase()))
        contacts = prefs.contacts
        viewModelScope.launch { watchContacts() }
    }

    fun removeContact(id: String) { prefs.removeContact(id); contacts = prefs.contacts }

    fun isGroup(chat: String) = chat.startsWith("g:")

    /** Группы: созданные здесь + восстановленные из истории (последний известный состав). */
    fun groups(): Map<String, Group> {
        val m = LinkedHashMap<String, Group>()
        runCatching {
            val a = JSONArray(prefs.localGroups)
            for (i in 0 until a.length()) {
                val o = a.getJSONObject(i)
                val mem = o.getJSONArray("members")
                m[o.getString("id")] = Group(o.getString("id"), o.getString("name"), (0 until mem.length()).map { mem.getString(it) })
            }
        }
        messages.filter { it.group != null && it.members.isNotEmpty() }.forEach {
            m[it.group!!] = Group(it.group, it.groupName ?: m[it.group]?.name ?: "Группа", it.members)
        }
        return m
    }

    fun createGroup(name: String, members: List<String>): String {
        val id = ByteArray(8).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
        val all = (members + myId).filter { it.isNotEmpty() }.distinct()
        val a = runCatching { JSONArray(prefs.localGroups) }.getOrElse { JSONArray() }
        a.put(JSONObject().put("id", id).put("name", name.trim()).put("members", JSONArray(all)))
        prefs.localGroups = a.toString()
        return "g:$id"
    }

    fun nameOf(chat: String): String {
        if (isGroup(chat)) return groups()[chat.removePrefix("g:")]?.name ?: "Группа"
        if (chat == myId) return "Вы"
        return contacts.firstOrNull { it.nodeId == chat }?.name ?: "${shortId(chat)}…"
    }

    fun chats(): List<ChatSummary> {
        val all = messages + pending
        val q = search.trim().lowercase()
        val list = all.groupBy { it.chat }.map { (chat, l) ->
            val last = l.maxBy { it.ts }
            val since = seen.value[chat] ?: 0L
            ChatSummary(chat, last, l.count { !it.out && it.ts > since })
        }.toMutableList()
        // группы, созданные здесь, видны и до первого сообщения
        groups().values.forEach { g ->
            if (list.none { it.chat == "g:${g.id}" })
                list += ChatSummary("g:${g.id}", ChatMsg("new-${g.id}", "g:${g.id}", "", true, "группа создана", 0, "sent", group = g.id), 0)
        }
        return list.filter { c ->
            q.isEmpty() || nameOf(c.chat).lowercase().contains(q) || c.chat.contains(q) ||
                all.any { it.chat == c.chat && it.text.lowercase().contains(q) }
        }.sortedByDescending { it.last.ts }
    }

    fun conversation(chat: String) = (messages.filter { it.chat == chat } + pending.filter { it.chat == chat }).sortedBy { it.ts }

    /** Беседа открыта: отметить прочитанным и (если включено) отправить отчёт о прочтении. */
    fun markSeen(chat: String) {
        val conv = messages.filter { it.chat == chat }
        val last = conv.maxOfOrNull { it.ts } ?: return
        if (!isGroup(chat) && prefs.sendReceipts) {
            val ids = conv.filter { !it.out && it.messageId.length == 32 && it.messageId !in reported && it.status == "received" &&
                it.ts > (seen.value[chat] ?: 0L) }.takeLast(50).map { it.messageId }
            if (ids.isNotEmpty()) {
                reported += ids
                viewModelScope.launch { runCatching { api.post("/api/read", mapOf("target" to chat, "ids" to ids.joinToString(","))) } }
            }
        }
        if ((seen.value[chat] ?: 0) >= last) return
        seen.value = seen.value + (chat to last)
        saveSeen(seen.value)
    }

    /** Набор текста: «печатает» не чаще раза в 4 с, только 1:1 и только по уже открытому туннелю. */
    fun onTyping(chat: String) {
        if (!prefs.sendTyping || isGroup(chat)) return
        val now = System.currentTimeMillis()
        if (now - (lastTypingSent[chat] ?: 0) < 4000) return
        lastTypingSent[chat] = now
        viewModelScope.launch { runCatching { api.post("/api/typing", mapOf("target" to chat), timeoutMs = 10_000) } }
    }

    /** NodeID или @псевдоним → NodeID (псевдоним разрешается через DHT и сохраняется в контакты). */
    suspend fun resolve(input: String): String? {
        val s = input.trim()
        if (isNodeId(s)) return s.lowercase()
        if (!s.startsWith("@") || s.length < 2) { toast("Нужен NodeID (64 hex) или @псевдоним"); return null }
        return runCatching { api.get("/api/resolve", mapOf("alias" to s.drop(1)), timeoutMs = 15000) }
            .fold({ r ->
                if (r.optBoolean("ok")) r.optString("node_id").also { id ->
                    if (contacts.none { it.nodeId == id }) saveContact(s, id)
                } else { toast("Псевдоним не найден: ${r.optString("error")}"); null }
            }, { toast("Ошибка: ${it.message}"); null })
    }

    fun send(chat: String, text: String) {
        val t = text.trim()
        if (t.isEmpty()) return
        val p = ChatMsg("p" + System.nanoTime(), chat, chat, true, t, System.currentTimeMillis(), "sending")
        pending += p
        lastTypingSent.remove(chat)
        viewModelScope.launch {
            val r = runCatching {
                if (isGroup(chat)) {
                    val g = groups()[chat.removePrefix("g:")] ?: error("группа не найдена")
                    api.post("/api/send_group", mapOf(
                        "group_id" to g.id, "name" to g.name, "text" to t,
                        "members" to g.members.filter { it != myId }.joinToString(","),
                    ), timeoutMs = 60_000)
                } else api.post("/api/send", mapOf("target" to chat, "text" to t), timeoutMs = 60_000)
            }
            refreshHistory()
            pending.remove(p)
            r.onSuccess {
                when {
                    !it.optBoolean("ok") -> toast("Не доставлено: ${it.optString("error")}")
                    it.optBoolean("queued") -> toast("Получатель недоступен — сообщение в почтовом ящике DHT")
                }
            }.onFailure { toast("Ошибка API: ${it.message}") }
        }
    }

    fun deleteChat(chat: String) {
        viewModelScope.launch {
            runCatching { api.post("/api/history/delete", mapOf("peer" to chat)) }
                .onSuccess { toast("Переписка удалена (${it.optInt("removed")} записей)") }
                .onFailure { toast("Ошибка: ${it.message}") }
            if (isGroup(chat)) {
                val id = chat.removePrefix("g:")
                val a = runCatching { JSONArray(prefs.localGroups) }.getOrElse { JSONArray() }
                val b = JSONArray()
                for (i in 0 until a.length()) if (a.getJSONObject(i).optString("id") != id) b.put(a.getJSONObject(i))
                prefs.localGroups = b.toString()
            }
            refreshHistory()
        }
    }

    // ── звонки ───────────────────────────────────────────────────────────

    // состояние звонка объявлено до init: его читает цикл опроса

    val callLive: Boolean get() = call?.optString("state").let { it == "ringing" || it == "active" }

    suspend fun refreshCalls() {
        val arr = runCatching { api.get("/api/calls").optJSONArray("sessions") }.getOrNull() ?: return
        val list = (0 until arr.length()).map { arr.getJSONObject(it) }
        val live = list.firstOrNull { it.optString("state") != "ended" }
        val now = System.currentTimeMillis()
        if (live != null) {
            if (call?.optString("id") != live.optString("id")) { callMinimized = false; muted = false; CallService.setMuted(false) }
            call = live; callEndedAt = 0
            return
        }
        val c = call ?: return
        if (callEndedAt == 0L) {
            callEndedAt = now
            call = list.firstOrNull { it.optString("id") == c.optString("id") } ?: JSONObject(c.toString()).put("state", "ended")
        } else if (now - callEndedAt > 2500) { call = null; callEndedAt = 0; callMinimized = false }
    }

    /** Название звонка: собеседник или «группа · кто звонит». */
    fun callTitle(c: JSONObject): String {
        val g = c.optJSONObject("group") ?: return nameOf(c.optJSONArray("peers")?.optJSONObject(0)?.optString("peer").orEmpty())
        return g.optString("name", "Групповой звонок")
    }

    fun startCall(chat: String) {
        if (callLive) { toast("Уже идёт звонок"); return }
        viewModelScope.launch {
            val r = runCatching {
                if (isGroup(chat)) {
                    val g = groups()[chat.removePrefix("g:")] ?: error("группа не найдена")
                    api.post("/api/call/start", mapOf("members" to g.members.filter { it != myId }.joinToString(","),
                        "name" to g.name, "chat" to g.id), timeoutMs = 60_000)
                } else api.post("/api/call/start", mapOf("target" to chat), timeoutMs = 60_000)
            }
            r.onSuccess {
                if (it.optBoolean("ok")) {
                    CallService.start(getApplication(), it.optString("call_id"), nameOf(chat))
                    refreshCalls()
                } else toast("Звонок не удался: ${it.optString("error")}")
            }.onFailure { toast("Ошибка: ${it.message}") }
        }
    }

    /** Убрать уведомление о входящем звонке (ответ, отказ или завершение — из приложения). */
    private fun clearIncoming(sid: String) =
        getApplication<Application>().getSystemService(android.app.NotificationManager::class.java)
            .cancel(NodeService.callNotifId(sid))

    fun acceptCall(sid: String) {
        clearIncoming(sid)
        viewModelScope.launch {
            val title = call?.let { callTitle(it) } ?: "Звонок"
            runCatching { api.post("/api/call/accept", mapOf("call_id" to sid), timeoutMs = 60_000) }
                .onSuccess {
                    if (it.optBoolean("ok")) CallService.start(getApplication(), sid, title)
                    else toast("Не удалось ответить: ${it.optString("error")}")
                }.onFailure { toast("Ошибка: ${it.message}") }
            refreshCalls()
        }
    }

    fun endCall(sid: String, reject: Boolean = false) {
        clearIncoming(sid)
        viewModelScope.launch {
            runCatching { api.post(if (reject) "/api/call/reject" else "/api/call/hangup", mapOf("call_id" to sid)) }
            refreshCalls()
        }
    }

    fun toggleMute() { muted = !muted; CallService.setMuted(muted) }
    fun toggleSpeaker() { speaker = !speaker; CallService.setSpeaker(speaker) }

    // ── безопасность ─────────────────────────────────────────────────────

    suspend fun safetyNumber(peer: String): String? =
        runCatching { api.get("/api/safety", mapOf("peer" to peer)).optString("safety") }.getOrNull()

    fun setVerified(peer: String, v: Boolean) {
        prefs.verified = if (v) prefs.verified + peer else prefs.verified - peer
        verified = prefs.verified
    }

    suspend fun route(peer: String): JSONObject? =
        runCatching { api.get("/api/route", mapOf("target" to peer), timeoutMs = 15_000) }.getOrNull()

    fun registerAlias(name: String) {
        val n = name.trim().removePrefix("@")
        if (n.isEmpty()) return
        viewModelScope.launch {
            runCatching { api.post("/api/alias", mapOf("name" to n), timeoutMs = 20000) }
                .onSuccess {
                    if (it.optBoolean("ok")) {
                        prefs.myAlias = n; myAlias = n
                        toast("Псевдоним @$n опубликован (${it.optInt("stored_on")} реплик)")
                    } else toast("Не удалось: ${it.optString("error")}")
                }
                .onFailure { toast("Ошибка: ${it.message}") }
        }
    }

    // ── файлы ────────────────────────────────────────────────────────────

    var sendingFile by mutableStateOf<String?>(null); private set

    /** Файл для показа в чате: принятый — в downloads узла, отправленный — копия в files/sent. */
    fun localFile(m: ChatMsg): File? {
        val name = m.fileName ?: return null
        val f = if (m.out) File(sentDir, name) else File(downloadsDir, name)
        return f.takeIf { downloadsDir.isNotEmpty() || m.out }?.takeIf { it.exists() }
    }

    /** Скопировать выбранный файл (узлу нужен путь) и отправить. Копия остаётся в files/sent для превью. */
    fun sendFile(peer: String, uri: Uri) {
        if (isGroup(peer)) { toast("Файлы — только в личных чатах"); return }
        val ctx = getApplication<Application>()
        viewModelScope.launch {
            val f = withContext(Dispatchers.IO) {
                runCatching {
                    // снимок камеры приходит как file:// (своя временная копия) — имя берём из файла
                    val name = if (uri.scheme == "file") File(uri.path!!).name
                        else ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                            ?.use { c -> if (c.moveToFirst()) c.getString(0) else null } ?: "file"
                    val out = File(sentDir, name.replace('/', '_'))
                    ctx.contentResolver.openInputStream(uri)!!.use { i -> out.outputStream().use { i.copyTo(it) } }
                    if (uri.scheme == "file") File(uri.path!!).delete()
                    out
                }.getOrNull()
            }
            if (f == null) { toast("Не удалось прочитать файл"); return@launch }
            sendingFile = f.name
            val r = runCatching {
                api.post("/api/send_file", mapOf("target" to peer, "path" to f.absolutePath), timeoutMs = 600_000)
            }
            sendingFile = null
            r.onSuccess {
                if (it.optBoolean("ok")) {
                    val rep = it.optJSONObject("report")
                    toast("«${f.name}» доставлен: %.2f МиБ/с".format(rep?.optDouble("goodput_mib_s") ?: 0.0))
                } else { toast("Файл не доставлен: ${it.optString("error")}"); f.delete() }
            }.onFailure { toast("Ошибка: ${it.message}") }
            refreshFiles(); refreshHistory()
        }
    }

    /** Файл для снимка камеры (FileProvider → камера пишет туда). */
    fun newPhotoFile(): File {
        val dir = File(getApplication<Application>().cacheDir, "camera").apply { mkdirs() }
        val ts = java.text.SimpleDateFormat("yyyyMMdd_HHmmss", java.util.Locale.US).format(java.util.Date())
        return File(dir, "photo_$ts.jpg")
    }

    fun logTail(n: Int = 200): List<String> = runCatching {
        NodeService.logFile(getApplication()).readLines().takeLast(n).map { NodeService.stripAnsi(it) }
    }.getOrDefault(emptyList())

    private fun loadSeen(): Map<String, Long> = runCatching {
        val o = JSONObject(getApplication<Application>().getSharedPreferences("p2p", 0).getString("seen", "{}")!!)
        o.keys().asSequence().associateWith { o.getLong(it) }
    }.getOrDefault(emptyMap())

    private fun saveSeen(m: Map<String, Long>) {
        getApplication<Application>().getSharedPreferences("p2p", 0).edit()
            .putString("seen", JSONObject(m as Map<*, *>).toString()).apply()
    }
}

fun JSONObject.optLongOrNull(k: String): Long? = if (has(k) && !isNull(k)) optLong(k) else null
fun JSONObject.optStringOrNull(k: String): String? = if (has(k) && !isNull(k)) optString(k).takeIf { it.isNotEmpty() } else null
