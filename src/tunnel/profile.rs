//! tunnel/profile.rs — профили кандидатов в ретрансляторы и стратегии выбора (ТЗ §12, продвинутый уровень).
//!
//! Профиль узла (по наблюдениям этого узла): число успехов и отказов, EMA успешности, EMA задержки,
//! время последнего наблюдения, число отказов подряд, подсеть. Обновляется при TUNNEL_BUILD (каждый hop
//! отдельно) и при передаче по туннелю (успех — всем hop; отказ — узлу, на котором оборвалась цепочка,
//! если его удаётся определить по ошибке; иначе всем hop туннеля).
//!
//! Стратегии (`tunnel.relay_selection`):
//! - `closest` — ближайшие к получателю по XOR (упрощённый уровень, по умолчанию);
//! - `random` — случайные допустимые узлы;
//! - `rating` — по убыванию оценки профиля; неизвестный узел получает нейтральную оценку (исследование);
//!   при равных — предпочтение разным подсетям.

use std::collections::HashMap;

use serde::Serialize;

use crate::types::{now_ms, Contact, NodeId};

const EMA_ALPHA: f64 = 0.3;
/// Оценка неизвестного узла: между надёжным (≈1) и отказывающим (≈0) — чтобы новые узлы пробовались.
const UNKNOWN_SCORE: f64 = 0.5;

#[derive(Debug, Clone, Serialize)]
pub struct RelayProfile {
    pub successes:   u32,
    pub failures:    u32,
    pub ema_success: f64,
    pub ema_latency_ms: f64,
    pub last_seen_ms: u64,
    pub consecutive_failures: u32,
    pub subnet:      String,
}

impl RelayProfile {
    fn new(subnet: String) -> Self {
        Self { successes: 0, failures: 0, ema_success: UNKNOWN_SCORE, ema_latency_ms: 0.0, last_seen_ms: 0, consecutive_failures: 0, subnet }
    }

    /// Оценка для стратегии `rating`: надёжность минус штраф за подряд идущие отказы и за задержку.
    pub fn score(&self) -> f64 {
        self.ema_success - 0.15 * self.consecutive_failures.min(4) as f64 - self.ema_latency_ms / 1000.0
    }
}

/// Подсеть /24 для IPv4 (иначе — хост целиком).
pub fn subnet_of(host: &str) -> String {
    match host.rsplit_once('.') {
        Some((net, _)) if host.parse::<std::net::Ipv4Addr>().is_ok() => format!("{net}.0/24"),
        _ => host.to_owned(),
    }
}

#[derive(Default)]
pub struct RelayProfiles {
    map: HashMap<NodeId, RelayProfile>,
}

impl RelayProfiles {
    fn entry(&mut self, c: &Contact) -> &mut RelayProfile {
        self.map.entry(c.node_id).or_insert_with(|| RelayProfile::new(subnet_of(&c.host)))
    }

    pub fn success(&mut self, c: &Contact, latency_ms: f64) {
        let p = self.entry(c);
        p.successes += 1;
        p.consecutive_failures = 0;
        p.ema_success = EMA_ALPHA + (1.0 - EMA_ALPHA) * p.ema_success;
        p.ema_latency_ms = if p.successes == 1 { latency_ms } else { EMA_ALPHA * latency_ms + (1.0 - EMA_ALPHA) * p.ema_latency_ms };
        p.last_seen_ms = now_ms();
    }

    pub fn failure(&mut self, c: &Contact) {
        let p = self.entry(c);
        p.failures += 1;
        p.consecutive_failures += 1;
        p.ema_success *= 1.0 - EMA_ALPHA;
        p.last_seen_ms = now_ms();
    }

    pub fn score(&self, id: &NodeId) -> f64 { self.map.get(id).map(|p| p.score()).unwrap_or(UNKNOWN_SCORE) }

    pub fn get(&self, id: &NodeId) -> Option<&RelayProfile> { self.map.get(id) }

    pub fn json(&self) -> serde_json::Value {
        let mut v: Vec<_> = self.map.iter().map(|(id, p)| serde_json::json!({ "node_id": id.to_hex(), "score": p.score(), "profile": p })).collect();
        v.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
        serde_json::Value::Array(v)
    }

    /// Выбрать `n` ретрансляторов из `cands` (уже без себя, получателя и исключённых), отсортированных по XOR к получателю.
    /// `avoid` — узлы других туннелей пула: используются, только если без них не набрать `n`.
    pub fn choose(&self, strategy: &str, mut cands: Vec<Contact>, n: usize, avoid: &std::collections::HashSet<NodeId>) -> Vec<Contact> {
        match strategy {
            "random" => {
                use rand::seq::SliceRandom;
                cands.shuffle(&mut rand::thread_rng());
            }
            "rating" => {
                // стабильная сортировка: при равной оценке сохраняется порядок по близости
                cands.sort_by(|a, b| self.score(&b.node_id).partial_cmp(&self.score(&a.node_id)).unwrap_or(std::cmp::Ordering::Equal));
            }
            _ => {}
        }
        let (fresh, used): (Vec<Contact>, Vec<Contact>) = cands.into_iter().partition(|c| !avoid.contains(&c.node_id));
        let ordered: Vec<Contact> = fresh.into_iter().chain(used).collect();
        if strategy != "rating" { return ordered.into_iter().take(n).collect(); }
        // rating: при прочих равных — разные подсети
        let mut out: Vec<Contact> = Vec::with_capacity(n);
        let mut nets = std::collections::HashSet::new();
        for c in &ordered {
            if out.len() == n { break; }
            if nets.insert(subnet_of(&c.host)) { out.push(c.clone()); }
        }
        for c in &ordered {
            if out.len() == n { break; }
            if !out.iter().any(|x| x.node_id == c.node_id) { out.push(c.clone()); }
        }
        out
    }
}

/// Кто виноват в обрыве цепочки. Ошибки вкладываются слева направо («first hop A … next hop B … connect to C»),
/// поэтому берётся самое правое упоминание: адрес после «connect to» / «handshake timeout to» (узел недоступен)
/// или после «first hop» / «next hop», если за ним следует отказ или тайм-аут.
pub fn blamed_addr(err: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for pat in ["connect to ", "handshake timeout to ", "first hop ", "next hop "] {
        let mut from = 0;
        while let Some(off) = err[from..].find(pat) {
            let pos = from + off;
            let rest = &err[pos + pat.len()..];
            let addr: String = rest.chars().take_while(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '[' | ']')).collect();
            let addr = addr.trim_end_matches(':').to_owned();
            let relevant = matches!(pat, "connect to " | "handshake timeout to ")
                || ["refused", "timeout", "отказал"].iter().any(|w| rest.contains(w));
            if addr.contains(':') && relevant && best.as_ref().map(|b| pos > b.0).unwrap_or(true) {
                best = Some((pos, addr));
            }
            from = pos + pat.len();
        }
    }
    best.map(|b| b.1)
}
