package org.p2poverlay.android

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.media.AudioAttributes
import android.media.RingtoneManager

class P2pApp : Application() {
    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CH_NODE, "Узел оверлея", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Постоянное уведомление, пока узел работает"
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CH_MSG, "Сообщения", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Входящие сообщения и файлы"
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CH_CALL, "Входящие звонки", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Звонок через оверлей: мелодия и экран вызова"
                setSound(RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE),
                    AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build())
                enableVibration(true)
                vibrationPattern = longArrayOf(0, 800, 600, 800, 600)
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CH_CALL_ONGOING, "Текущий звонок", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Пока идёт звонок: длительность и кнопка «Завершить»"
            }
        )
    }

    companion object {
        const val CH_NODE = "node"
        const val CH_MSG = "messages"
        const val CH_CALL = "calls_incoming"
        const val CH_CALL_ONGOING = "calls_ongoing"
    }
}
