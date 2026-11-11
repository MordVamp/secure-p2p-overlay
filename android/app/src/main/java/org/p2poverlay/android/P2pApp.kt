package org.p2poverlay.android

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager

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
    }

    companion object {
        const val CH_NODE = "node"
        const val CH_MSG = "messages"
    }
}
