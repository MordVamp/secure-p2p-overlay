package org.p2poverlay.android

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/**
 * Клиент локального HTTP API узла (`p2p-node --gui-port`, только 127.0.0.1) —
 * тот же API, что у веб-панели и desktop-приложения.
 */
class NodeApi(private val port: () -> Int, private val token: () -> String? = { null }) {
    class ApiError(msg: String) : Exception(msg)

    suspend fun get(path: String, q: Map<String, String> = emptyMap(), timeoutMs: Int = 5000) =
        call("GET", path, q, timeoutMs)

    suspend fun post(path: String, q: Map<String, String> = emptyMap(), timeoutMs: Int = 30000) =
        call("POST", path, q, timeoutMs)

    private suspend fun call(method: String, path: String, q: Map<String, String>, timeoutMs: Int): JSONObject =
        withContext(Dispatchers.IO) {
            val qs = q.entries.joinToString("&") { (k, v) ->
                URLEncoder.encode(k, "UTF-8") + "=" + URLEncoder.encode(v, "UTF-8")
            }
            val url = URL("http://127.0.0.1:${port()}$path" + if (qs.isEmpty()) "" else "?$qs")
            val c = url.openConnection() as HttpURLConnection
            try {
                c.requestMethod = method
                c.connectTimeout = 2000
                c.readTimeout = timeoutMs
                c.useCaches = false
                token()?.let { c.setRequestProperty("X-Api-Token", it) }
                if (method == "POST") { c.doOutput = true; c.setFixedLengthStreamingMode(0); c.outputStream.close() }
                val code = c.responseCode
                val body = (if (code < 400) c.inputStream else c.errorStream)?.bufferedReader()?.use { it.readText() } ?: ""
                val json = runCatching { JSONObject(body) }.getOrElse { throw ApiError("HTTP $code: $body") }
                if (code >= 400) throw ApiError(json.optString("error", "HTTP $code"))
                json
            } finally {
                c.disconnect()
            }
        }

    suspend fun alive(): Boolean = runCatching { get("/api/status", timeoutMs = 1500) }.isSuccess

    companion object {
        /** Клиент API узла этого приложения (порт из настроек, токен из Secrets). */
        fun of(ctx: android.content.Context): NodeApi {
            val p = Prefs(ctx)
            val t = Secrets.apiToken(ctx.applicationContext)
            return NodeApi({ p.apiPort }, { t })
        }
    }
}
