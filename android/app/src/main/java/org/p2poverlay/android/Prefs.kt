package org.p2poverlay.android

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

data class Contact(val name: String, val nodeId: String)

/**
 * Режимы энергопотребления. Что меняется — и в узле, и в приложении:
 *  - ECO: «только клиент» (чужой трафик не ретранслируется), без wake lock (процессор засыпает,
 *    входящий пакет его будит), обновление DHT раз в 10 мин, почтовые ящики — раз в 5 мин.
 *  - NORMAL: ретранслятор, без wake lock, обычные интервалы.
 *  - MAX: ретранслятор с wake lock (всегда отвечает на PING и держит чужие туннели),
 *    пул из 2 туннелей к собеседнику (мгновенное переключение при отказе), ящики — раз в 30 с.
 * Во всех режимах интерфейс и уведомления получают события push-запросом `/api/events`.
 */
enum class PowerMode(val id: String, val title: String) {
    ECO("eco", "Эконом"), NORMAL("normal", "Обычный"), MAX("max", "Максимум");

    /** Флаги узла для режима. */
    fun nodeArgs(clientOnly: Boolean): List<String> = buildList {
        if (clientOnly || this@PowerMode == ECO) add("--no-relay")
        when (this@PowerMode) {
            ECO -> addAll(listOf("--refresh-interval", "600", "--mailbox-poll", "300"))
            NORMAL -> addAll(listOf("--mailbox-poll", "60"))
            MAX -> addAll(listOf("--pool-size", "2", "--mailbox-poll", "30"))
        }
    }
    val wakeLock: Boolean get() = this == MAX
    /** Как часто интерфейс обновляет сводку сети (события приходят push'ем независимо). */
    val statusPollMs: Long get() = when (this) { ECO -> 15_000; NORMAL -> 5_000; MAX -> 3_000 }

    companion object { fun of(id: String) = entries.firstOrNull { it.id == id } ?: NORMAL }
}

/** Настройки приложения и адресная книга (SharedPreferences). */
class Prefs(ctx: Context) {
    private val sp = ctx.getSharedPreferences("p2p", Context.MODE_PRIVATE)

    var listenPort: Int
        get() = sp.getInt("listen_port", 7600)
        set(v) = sp.edit().putInt("listen_port", v).apply()
    var apiPort: Int
        get() = sp.getInt("api_port", 7690)
        set(v) = sp.edit().putInt("api_port", v).apply()
    /** Bootstrap-пиры через запятую: адрес узла на компьютере (из QR) и др. */
    var bootstrap: String
        get() = sp.getString("bootstrap", "") ?: ""
        set(v) = sp.edit().putString("bootstrap", v).apply()
    /** tls | ake — должен совпадать с сетью. */
    var channel: String
        get() = sp.getString("channel", "tls") ?: "tls"
        set(v) = sp.edit().putString("channel", v).apply()
    /** Анонсировать Wi-Fi-адрес телефона (иначе к нему не смогут подключиться другие узлы). */
    var lan: Boolean
        get() = sp.getBoolean("lan", true)
        set(v) = sp.edit().putBoolean("lan", v).apply()
    var autostart: Boolean
        get() = sp.getBoolean("autostart", true)
        set(v) = sp.edit().putBoolean("autostart", v).apply()
    var myAlias: String
        get() = sp.getString("my_alias", "") ?: ""
        set(v) = sp.edit().putString("my_alias", v).apply()
    /** Режим энергопотребления: eco | normal | max (см. PowerMode). */
    var powerMode: String
        get() = sp.getString("power_mode", "normal") ?: "normal"
        set(v) = sp.edit().putString("power_mode", v).apply()
    /** «Только клиент»: не ретранслировать чужие туннели (в режиме «Эконом» — всегда). */
    var clientOnly: Boolean
        get() = sp.getBoolean("client_only", false)
        set(v) = sp.edit().putBoolean("client_only", v).apply()
    /** Сообщать собеседнику «печатает…». */
    var sendTyping: Boolean
        get() = sp.getBoolean("send_typing", true)
        set(v) = sp.edit().putBoolean("send_typing", v).apply()
    /** Отправлять отчёты о прочтении. */
    var sendReceipts: Boolean
        get() = sp.getBoolean("send_receipts", true)
        set(v) = sp.edit().putBoolean("send_receipts", v).apply()
    /** Тема: system | light | dark. */
    var theme: String
        get() = sp.getString("theme", "system") ?: "system"
        set(v) = sp.edit().putString("theme", v).apply()
    /** Блокировка приложения биометрией / PIN устройства. */
    var biometricLock: Boolean
        get() = sp.getBoolean("biometric", false)
        set(v) = sp.edit().putBoolean("biometric", v).apply()
    /** Собеседники, чей номер безопасности сверен лично. */
    var verified: Set<String>
        get() = sp.getStringSet("verified", emptySet()) ?: emptySet()
        set(v) = sp.edit().putStringSet("verified", v).apply()
    /** Созданные здесь группы (до первого сообщения их нет в истории): JSON [{id,name,members}]. */
    var localGroups: String
        get() = sp.getString("groups", "[]") ?: "[]"
        set(v) = sp.edit().putString("groups", v).apply()

    val mode: PowerMode get() = PowerMode.of(powerMode)
    /** Итоговый «только клиент»: включён вручную или режимом «Эконом». */
    val effectiveClientOnly: Boolean get() = clientOnly || mode == PowerMode.ECO

    var notifyMessages: Boolean
        get() = sp.getBoolean("notify", true)
        set(v) = sp.edit().putBoolean("notify", v).apply()
    /** Время последнего показанного входящего (для уведомлений без повторов). */
    var lastNotifiedTs: Long
        get() = sp.getLong("last_notified", 0)
        set(v) = sp.edit().putLong("last_notified", v).apply()

    var contacts: List<Contact>
        get() {
            val arr = runCatching { JSONArray(sp.getString("contacts", "[]")) }.getOrElse { JSONArray() }
            return (0 until arr.length()).map {
                val o = arr.getJSONObject(it)
                Contact(o.optString("name"), o.optString("node_id"))
            }
        }
        set(v) {
            val arr = JSONArray()
            v.forEach { arr.put(JSONObject().put("name", it.name).put("node_id", it.nodeId)) }
            sp.edit().putString("contacts", arr.toString()).apply()
        }

    fun upsertContact(c: Contact) {
        contacts = contacts.filter { it.nodeId != c.nodeId } + c
    }

    fun removeContact(nodeId: String) {
        contacts = contacts.filter { it.nodeId != nodeId }
    }
}

/** Ссылка-приглашение `p2p-overlay://host:port?id=<64 hex>` (QR на компьютере). */
data class Invite(val addr: String, val nodeId: String?) {
    companion object {
        fun parse(raw: String): Invite? {
            val s = raw.trim()
            val body = s.removePrefix("p2p-overlay://")
            val addr = body.substringBefore('?').trimEnd('/')
            if (!Regex("""^[A-Za-z0-9.\-]+:\d{1,5}$""").matches(addr)) return null
            val id = Regex("""(?:^|[?&])id=([0-9a-fA-F]{64})""").find(body)?.groupValues?.get(1)?.lowercase()
            return Invite(addr, id)
        }
    }
}

fun isNodeId(s: String) = Regex("^[0-9a-fA-F]{64}$").matches(s.trim())
fun shortId(id: String) = if (id.length > 8) id.take(8) else id
