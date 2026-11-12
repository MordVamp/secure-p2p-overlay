package org.p2poverlay.android

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File
import java.net.Inet4Address

sealed class RunState {
    data object Stopped : RunState()
    data object Starting : RunState()
    data class Running(val pid: Int?) : RunState()
    data class Failed(val reason: String) : RunState()
}

/**
 * Foreground-сервис, который держит запущенным настоящий узел оверлея — `p2p-node`,
 * собранный из того же исходного кода, что и на компьютере (Rust, aarch64/x86_64-linux-android).
 * Бинарник лежит в APK как `libp2pnode.so` (Android разрешает exec только из nativeLibraryDir).
 * Интерфейс общается с узлом через его локальный HTTP API на 127.0.0.1.
 */
class NodeService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var proc: Process? = null
    @Volatile private var stopping = false
    private var wake: PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> { stopNode(); stopForeground(STOP_FOREGROUND_REMOVE); stopSelf(); return START_NOT_STICKY }
            ACTION_CALL_REJECT -> {
                val sid = intent.getStringExtra(EXTRA_SID).orEmpty()
                getSystemService(NotificationManager::class.java).cancel(callNotifId(sid))
                scope.launch { runCatching { NodeApi.of(this@NodeService).post("/api/call/reject", mapOf("call_id" to sid)) } }
                return START_STICKY
            }
            else -> {
                startInForeground("Запуск узла…")
                if (proc?.isAlive != true) scope.launch { startNode() }
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        netCallback?.let { runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) } }
        stopNode()
        scope.cancel()
        super.onDestroy()
    }

    private fun startInForeground(text: String) {
        val n = ongoing(text)
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTIF_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTIF_ID, n)
    }

    private fun ongoing(text: String): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop = PendingIntent.getService(this, 1, Intent(this, NodeService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE)
        return NotificationCompat.Builder(this, P2pApp.CH_NODE)
            .setSmallIcon(R.drawable.ic_stat_node)
            .setContentTitle("Узел оверлея")
            .setContentText(text)
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(0, "Остановить", stop)
            .build()
    }

    private fun updateOngoing(text: String) =
        getSystemService(NotificationManager::class.java).notify(NOTIF_ID, ongoing(text))

    private suspend fun startNode() {
        val prefs = Prefs(this)
        val api = NodeApi.of(this)
        _state.value = RunState.Starting
        stopping = false
        killOrphan(prefs, api)

        val bin = File(applicationInfo.nativeLibraryDir, "libp2pnode.so")
        if (!bin.exists()) { fail("в APK нет узла (${bin.name}) для ABI ${Build.SUPPORTED_ABIS.first()}"); return }
        val state = stateDir(this).apply { mkdirs() }
        val cfg = File(filesDir, "node_config.yaml")
        assets.open("node_config.yaml").use { i -> cfg.outputStream().use { i.copyTo(it) } }

        val args = mutableListOf(
            bin.absolutePath, "-c", cfg.absolutePath,
            "-p", prefs.listenPort.toString(), "--gui-port", prefs.apiPort.toString(),
            "--state-dir", state.absolutePath, "--metrics-dir", File(state, "metrics").absolutePath,
            "--channel", prefs.channel,
        )
        advertised = if (prefs.lan) (wifiIp(this) ?: "auto") else null
        advertised?.let { args += listOf("--advertise", it) }
        args += prefs.mode.nodeArgs(prefs.clientOnly)
        prefs.bootstrap.split(',', ' ', ';').map { it.trim() }.filter { it.isNotEmpty() }
            .forEach { args += listOf("-b", it) }

        val log = logFile(this)
        val p = try {
            ProcessBuilder(args).directory(filesDir).redirectErrorStream(true)
                .redirectOutput(ProcessBuilder.Redirect.to(log))
                .apply {
                    // секреты — через окружение (не видны в списке процессов); узел удаляет их у себя после чтения
                    environment()["P2P_API_TOKEN"] = Secrets.apiToken(this@NodeService)
                    environment()["P2P_IDENTITY_KEK"] = Secrets.identityKek(this@NodeService)
                }
                .start()
        } catch (e: Exception) { fail("не удалось запустить узел: ${e.message}"); return }
        proc = p
        val pid = pidOf(p)
        pid?.let { File(filesDir, "node.pid").writeText(it.toString()) }
        if (prefs.mode.wakeLock) acquireWake()

        // ждём, пока поднимется API (или процесс упадёт)
        val t0 = System.currentTimeMillis()
        while (System.currentTimeMillis() - t0 < 20_000) {
            if (!p.isAlive) { fail("узел завершился: " + lastLines(log, 3)); return }
            if (api.alive()) break
            delay(250)
        }
        if (!api.alive()) { fail("API узла не ответил за 20 с"); return }
        _state.value = RunState.Running(pid)
        val role = if (prefs.effectiveClientOnly) "только клиент" else "ретранслятор"
        updateOngoing("${prefs.mode.title} · $role · порт ${prefs.listenPort}")
        watchNetwork(prefs)

        scope.launch { // процесс узла завершился сам
            val code = p.waitFor()
            if (!stopping) fail("узел завершился (код $code): " + lastLines(log, 3))
        }
        scope.launch { notifyLoop(prefs, api) }
    }

    private fun fail(reason: String) {
        _state.value = RunState.Failed(reason)
        releaseWake()
        updateOngoing("Ошибка: $reason")
    }

    private fun stopNode() {
        stopping = true
        proc?.let { p ->
            p.destroy()
            runCatching { if (!p.waitFor(3, java.util.concurrent.TimeUnit.SECONDS)) p.destroyForcibly() }
        }
        proc = null
        File(filesDir, "node.pid").delete()
        releaseWake()
        _state.value = RunState.Stopped
    }

    /** Узел от прошлого запуска приложения (процесс приложения убит, узел остался) — остановить. */
    private suspend fun killOrphan(prefs: Prefs, api: NodeApi) {
        val f = File(filesDir, "node.pid")
        val pid = f.takeIf { it.exists() }?.readText()?.trim()?.toIntOrNull() ?: return
        if (api.alive()) {
            android.os.Process.killProcess(pid)
            repeat(20) { if (!api.alive()) return@repeat; delay(150) }
        }
        f.delete()
    }

    /**
     * Уведомления о входящих — push: один «долгий» запрос /api/events (узел отвечает, как только
     * пришло сообщение, иначе через 25 с). Процессор не просыпается каждые 2 с, как при опросе.
     */
    private suspend fun notifyLoop(prefs: Prefs, api: NodeApi) {
        val nm = getSystemService(NotificationManager::class.java)
        var since = runCatching { api.get("/api/status").optLong("events_seq") }.getOrDefault(0L)
        while (scope.isActive && _state.value is RunState.Running) {
            val r = runCatching {
                api.get("/api/events", mapOf("since" to since.toString(), "timeout" to "25"), timeoutMs = 35_000)
            }.getOrElse { delay(3000); continue }
            since = r.optLong("seq", since)
            val ev = r.optJSONArray("events") ?: continue
            for (i in 0 until ev.length()) {
                val e = ev.getJSONObject(i)
                if (e.optString("kind") == "call") { incomingCall(nm, prefs, e); continue }
                if (e.optString("kind") != "message") continue
                if (MainActivity.visible || !prefs.notifyMessages) continue
                val peer = e.optString("peer")
                val group = e.optString("group").takeIf { it.isNotEmpty() && it != "null" }
                val who = prefs.contacts.firstOrNull { it.nodeId == peer }?.name ?: "${shortId(peer)}…"
                val chat = if (group != null) "g:$group" else peer
                val title = if (group != null) "${e.optString("group_name")} · $who" else who
                val open = PendingIntent.getActivity(this, chat.hashCode(),
                    Intent(this, MainActivity::class.java).putExtra(MainActivity.EXTRA_PEER, chat),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
                val via = if (e.optBoolean("via_mailbox")) " (из почтового ящика)" else ""
                nm.notify(chat.hashCode(), NotificationCompat.Builder(this, P2pApp.CH_MSG)
                    .setSmallIcon(R.drawable.ic_stat_node)
                    .setContentTitle(title)
                    .setContentText(e.optString("text", "сообщение") + via)
                    .setAutoCancel(true)
                    .setContentIntent(open)
                    .build())
            }
        }
    }

    /**
     * Входящий звонок: уведомление в стиле вызова (мелодия, «Ответить» / «Отклонить», на заблокированном
     * экране — полноэкранный вызов). Когда звонок принят, отклонён или закончился — уведомление снимается.
     */
    private fun incomingCall(nm: NotificationManager, prefs: Prefs, e: org.json.JSONObject) {
        val sid = e.optString("id")
        val ringing = e.optString("state") == "ringing" && !e.optBoolean("outgoing")
        if (!ringing) { nm.cancel(callNotifId(sid)); return }
        val group = e.optJSONObject("group")
        val peers = e.optJSONArray("peers")
        val peer = group?.optString("initiator") ?: peers?.optJSONObject(0)?.optString("peer").orEmpty()
        val who = prefs.contacts.firstOrNull { it.nodeId == peer }?.name ?: "${shortId(peer)}…"
        val title = if (group != null) "${group.optString("name")} · $who" else who
        val answer = PendingIntent.getActivity(this, sid.hashCode(),
            Intent(this, MainActivity::class.java).putExtra(MainActivity.EXTRA_CALL, sid).putExtra(MainActivity.EXTRA_CALL_ACCEPT, true)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val full = PendingIntent.getActivity(this, sid.hashCode() + 1,
            Intent(this, MainActivity::class.java).putExtra(MainActivity.EXTRA_CALL, sid)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val decline = PendingIntent.getService(this, sid.hashCode() + 2,
            Intent(this, NodeService::class.java).setAction(ACTION_CALL_REJECT).putExtra(EXTRA_SID, sid),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val person = androidx.core.app.Person.Builder().setName(title).setImportant(true).build()
        nm.notify(callNotifId(sid), NotificationCompat.Builder(this, P2pApp.CH_CALL)
            .setSmallIcon(R.drawable.ic_stat_node)
            .setStyle(NotificationCompat.CallStyle.forIncomingCall(person, decline, answer))
            .setContentTitle(title)
            .setContentText(if (group != null) "Групповой звонок через оверлей" else "Звонок через оверлей · E2E")
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setFullScreenIntent(full, true)
            .setContentIntent(full)
            .setOngoing(true)
            .setTimeoutAfter(35_000)
            .build())
    }

    // ── смена сети: Wi-Fi ↔ мобильная, новый адрес — узел перезапускается с новым --advertise ──
    @Volatile private var advertised: String? = null
    private var netCallback: ConnectivityManager.NetworkCallback? = null
    private var restartJob: kotlinx.coroutines.Job? = null

    private fun watchNetwork(prefs: Prefs) {
        if (netCallback != null) return
        val cm = getSystemService(ConnectivityManager::class.java)
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onLinkPropertiesChanged(network: android.net.Network, lp: android.net.LinkProperties) = check()
            override fun onAvailable(network: android.net.Network) = check()
            private fun check() {
                if (!prefs.lan || _state.value !is RunState.Running) return
                val ip = wifiIp(this@NodeService) ?: return
                if (ip == advertised) return
                // дребезг: при переключении сети событий несколько подряд
                restartJob?.cancel()
                restartJob = scope.launch {
                    delay(3000)
                    if (wifiIp(this@NodeService) == advertised) return@launch
                    updateOngoing("Сеть сменилась — перезапуск узла с адресом $ip")
                    stopNode()
                    delay(800)
                    startNode()
                }
            }
        }
        cm.registerDefaultNetworkCallback(cb)
        netCallback = cb
    }

    private fun acquireWake() {
        // частичная блокировка сна: узел — ретранслятор для чужих туннелей и должен отвечать на PING
        wake = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "p2p:node").apply { setReferenceCounted(false); acquire() }
    }

    private fun releaseWake() { runCatching { wake?.release() }; wake = null }

    companion object {
        const val ACTION_START = "start"
        const val ACTION_STOP = "stop"
        const val ACTION_CALL_REJECT = "call_reject"
        const val EXTRA_SID = "sid"
        fun callNotifId(sid: String) = 1000 + (sid.hashCode() and 0xffff)
        private const val NOTIF_ID = 1

        private val _state = MutableStateFlow<RunState>(RunState.Stopped)
        val state: StateFlow<RunState> = _state

        fun start(ctx: Context) =
            ContextCompat.startForegroundService(ctx, Intent(ctx, NodeService::class.java).setAction(ACTION_START))

        fun stop(ctx: Context) {
            ctx.startService(Intent(ctx, NodeService::class.java).setAction(ACTION_STOP))
        }

        fun stateDir(ctx: Context) = File(ctx.filesDir, "node")
        fun logFile(ctx: Context) = File(ctx.filesDir, "node.log")

        /** IPv4 активной сети (обычно Wi-Fi) — этот адрес узел сообщает пирам. */
        fun wifiIp(ctx: Context): String? {
            val cm = ctx.getSystemService(ConnectivityManager::class.java)
            val lp = cm.getLinkProperties(cm.activeNetwork) ?: return null
            return lp.linkAddresses.map { it.address }
                .firstOrNull { it is Inet4Address && !it.isLoopbackAddress }?.hostAddress
        }

        fun lastLines(f: File, n: Int): String =
            runCatching { f.readLines().takeLast(n).joinToString(" | ") { stripAnsi(it) } }.getOrDefault("")

        fun stripAnsi(s: String) = s.replace(Regex("\u001B\\[[0-9;]*[A-Za-z]"), "")

        private fun pidOf(p: Process): Int? = runCatching {
            val f = p.javaClass.getDeclaredField("pid"); f.isAccessible = true; f.getInt(p)
        }.getOrNull()
    }
}
