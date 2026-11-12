package org.p2poverlay.android

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.Person
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/**
 * Идёт звонок: foreground-сервис с микрофоном (Android не даёт записывать звук из фона без него), звук через
 * [CallAudio], уведомление с таймером и кнопкой «Завершить», датчик приближения у уха (экран гаснет).
 * Запускается действием пользователя (позвонить / ответить); сам останавливается, когда сеанс закончился.
 */
class CallService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var audio: CallAudio? = null
    private var prox: PowerManager.WakeLock? = null
    private var sid: String = ""
    private var title: String = "Звонок"

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_HANGUP -> {
                val id = sid
                scope.launch { runCatching { NodeApi.of(this@CallService).post("/api/call/hangup", mapOf("call_id" to id)) } }
                finish()
                return START_NOT_STICKY
            }
            else -> {
                val newSid = intent?.getStringExtra(EXTRA_SID).orEmpty()
                title = intent?.getStringExtra(EXTRA_TITLE) ?: title
                foreground("Соединение…")
                if (newSid != sid || audio == null) {
                    sid = newSid
                    scope.launch { run() }
                }
            }
        }
        return START_NOT_STICKY
    }

    private fun foreground(text: String) {
        val n = notification(text)
        val mic = audio?.hasMic ?: (ContextCompat.checkSelfPermission(this, android.Manifest.permission.RECORD_AUDIO) ==
            android.content.pm.PackageManager.PERMISSION_GRANTED)
        if (Build.VERSION.SDK_INT >= 30) {
            val type = if (mic) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE else ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            startForeground(NOTIF_ID, n, type)
        } else startForeground(NOTIF_ID, n)
    }

    private fun notification(text: String): Notification {
        val open = PendingIntent.getActivity(this, 2, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val hang = PendingIntent.getService(this, 3, Intent(this, CallService::class.java).setAction(ACTION_HANGUP),
            PendingIntent.FLAG_IMMUTABLE)
        return NotificationCompat.Builder(this, P2pApp.CH_CALL_ONGOING)
            .setSmallIcon(R.drawable.ic_stat_node)
            .setStyle(NotificationCompat.CallStyle.forOngoingCall(Person.Builder().setName(title).build(), hang))
            .setContentText(text)
            .setOngoing(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setContentIntent(open)
            .build()
    }

    private suspend fun run() {
        val api = NodeApi.of(this)
        val link = runCatching { api.get("/api/call/audio") }.getOrNull()
        if (link == null || !link.optBoolean("ok")) { finish(); return }
        val token = link.optString("token").chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        val a = CallAudio(this, link.optInt("port"), token)
        a.muted = pendingMute; a.speaker = pendingSpeaker
        a.start()
        audio = a
        current.value = a
        setProximity(!a.speaker)
        var missing = 0
        val nm = getSystemService(NotificationManager::class.java)
        while (audio === a) {
            val s = runCatching {
                val arr = api.get("/api/calls").optJSONArray("sessions")
                (0 until (arr?.length() ?: 0)).map { arr!!.getJSONObject(it) }.firstOrNull { it.optString("id") == sid }
            }.getOrNull()
            if (s == null) { if (++missing > 5) break } else missing = 0
            val state = s?.optString("state")
            if (state != "ringing" || s.optBoolean("outgoing")) nm.cancel(NodeService.callNotifId(sid))
            if (state == "ended") break
            val dur = s?.optLong("duration_s", -1) ?: -1
            val text = when {
                state == "ringing" && s.optBoolean("outgoing") -> "Вызов…"
                dur >= 0 -> "%d:%02d".format(dur / 60, dur % 60)
                else -> "Соединение…"
            }
            nm.notify(NOTIF_ID, notification(text))
            setProximity(!a.speaker)
            delay(1000)
        }
        finish()
    }

    private fun setProximity(on: Boolean) {
        val pm = getSystemService(PowerManager::class.java)
        if (!pm.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) return
        if (on && prox == null) prox = pm.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "p2p:call").apply {
            setReferenceCounted(false); acquire(4 * 60 * 60 * 1000L)
        }
        if (!on) { runCatching { prox?.release() }; prox = null }
    }

    private fun finish() {
        audio?.stop()
        audio = null
        current.value = null
        setProximity(false)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        audio?.stop(); audio = null; current.value = null
        setProximity(false)
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        const val ACTION_START = "start"
        const val ACTION_HANGUP = "hangup"
        const val EXTRA_SID = "sid"
        const val EXTRA_TITLE = "title"
        private const val NOTIF_ID = 3

        private val current = MutableStateFlow<CallAudio?>(null)
        /** Звук текущего звонка (для кнопок «микрофон» и «динамик» и индикатора громкости). */
        val audio: StateFlow<CallAudio?> = current
        @Volatile var pendingMute = false
        @Volatile var pendingSpeaker = false

        fun start(ctx: Context, sid: String, title: String) = ContextCompat.startForegroundService(ctx,
            Intent(ctx, CallService::class.java).setAction(ACTION_START).putExtra(EXTRA_SID, sid).putExtra(EXTRA_TITLE, title))

        fun setMuted(v: Boolean) { pendingMute = v; current.value?.muted = v }
        fun setSpeaker(v: Boolean) { pendingSpeaker = v; current.value?.speaker = v }
    }
}
