package org.p2poverlay.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.p2poverlay.android.AppViewModel

private fun JSONArray?.objs() = if (this == null) emptyList() else (0 until length()).map { getJSONObject(it) }

@Composable
fun NetworkScreen(vm: AppViewModel) {
    val st = vm.status
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (st == null) { Text("Узел не запущен", color = P.Muted); return@Column }
        val routing = st.optJSONObject("routing")
        val tunnels = st.optJSONObject("tunnels")
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Stat(Modifier.weight(1f), "${routing?.optInt("total_contacts") ?: 0}", "пиров")
            Stat(Modifier.weight(1f), "${tunnels?.optInt("total_tunnels") ?: 0}", "туннелей")
            Stat(Modifier.weight(1f), "${st.optJSONArray("store")?.length() ?: 0}", "записей DHT")
        }
        SectionCard("Этот узел") {
            KeyValue("NodeID", st.optString("node_id"), mono = true)
            KeyValue("Адрес для пиров", st.optString("addr"))
            KeyValue("Канал", if (st.optString("channel") == "ake") "AKE (своё рукопожатие)" else "TLS 1.3")
            KeyValue("Уровень", st.optString("level"))
            KeyValue("k / α", "${st.optInt("k")} / ${st.optInt("alpha")}")
            KeyValue("Роль", if (st.optBoolean("relay", true)) "ретранслятор чужих туннелей" else "только клиент (чужой трафик не пересылает)")
            KeyValue("Режим", vm.prefs.mode.title)
        }
        SectionCard("Туннели · пул ${tunnels?.optInt("pool_size") ?: 1} · ${tunnels?.optString("relay_selection") ?: ""}") {
            val list = tunnels?.optJSONArray("tunnels").objs()
            if (list.isEmpty()) Text("Нет активных туннелей — появятся при первой отправке", color = P.Muted, fontSize = 13.sp)
            list.forEach { t ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("→ ${t.optString("target")}…", fontFamily = FontFamily.Monospace, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    Text("${t.optInt("alive_hops")}/${t.optInt("hops")} хопов", color = P.Muted, fontSize = 12.sp)
                    val s = t.optString("state")
                    Pill(if (t.optString("role") == "backup") "$s · резерв" else s,
                        when (s) { "Active" -> P.Green; "Degraded", "Building" -> P.Yellow; else -> P.Red })
                }
            }
        }
        SectionCard("Таблица маршрутизации (Kademlia)") {
            val buckets = routing?.optJSONArray("buckets").objs().filter { (it.optJSONArray("contacts")?.length() ?: 0) > 0 }
            if (buckets.isEmpty()) Text("Пусто — укажите bootstrap (адрес компьютера)", color = P.Yellow, fontSize = 13.sp)
            buckets.forEach { b ->
                Text("Бакет ${b.optInt("bucket_index")}", fontWeight = FontWeight.SemiBold, fontSize = 13.sp)
                b.optJSONArray("contacts").objs().forEach { c ->
                    val id = c.optString("node_id")
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Avatar(id, vm.nameOf(id), 26.dp)
                        Text(vm.nameOf(id), fontSize = 13.sp, modifier = Modifier.weight(1f))
                        if (!c.optBoolean("relay", true)) Pill("клиент", P.Yellow)
                        Text("${c.optString("host")}:${c.optInt("port")}", color = P.Muted, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                    }
                }
                HorizontalDivider(color = P.Side)
            }
        }
        SectionCard("Профили ретрансляторов") {
            val rel = vm.relays.objs()
            if (rel.isEmpty()) Text("Нет данных — копятся по мере построения туннелей", color = P.Muted, fontSize = 13.sp)
            rel.take(12).forEach { r ->
                val id = r.optString("node_id")
                val p = r.optJSONObject("profile")
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(vm.nameOf(id), fontSize = 13.sp, modifier = Modifier.weight(1f))
                    Text("✓${p?.optInt("successes") ?: 0} ✗${p?.optInt("failures") ?: 0}", color = P.Muted, fontSize = 12.sp)
                    Pill("%.2f".format(r.optDouble("score")), if (r.optDouble("score") >= 0.5) P.Green else P.Yellow)
                }
            }
        }
    }
}

@Composable
private fun Stat(modifier: Modifier, value: String, label: String) {
    SectionCard(modifier = modifier) {
        Text(value, fontSize = 26.sp, fontWeight = FontWeight.SemiBold)
        Text(label, color = P.Muted, fontSize = 12.sp)
    }
}
