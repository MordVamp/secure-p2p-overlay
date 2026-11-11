//! app/events.rs — события узла для клиентов (push вместо опроса).
//!
//! Кольцевой буфер последних событий с порядковым номером. Клиент (Android, desktop) держит
//! один «долгий» запрос `GET /api/events?since=N&timeout=25`: ответ приходит сразу, как только
//! появилось событие с номером > N, иначе — по тайм-ауту с пустым списком. Это дешевле опроса
//! раз в секунду и даёт мгновенные уведомления.

use std::collections::VecDeque;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{watch, Mutex};

use crate::types::now_ms;

const CAP: usize = 256;

pub struct EventBus {
    buf: Mutex<VecDeque<(u64, Value)>>,
    seq: watch::Sender<u64>,
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

impl EventBus {
    pub fn new() -> Self { Self { buf: Mutex::new(VecDeque::new()), seq: watch::channel(0).0 } }

    /// Опубликовать событие: `kind` + произвольные поля.
    pub async fn emit(&self, kind: &str, mut fields: Value) {
        let mut b = self.buf.lock().await;
        let n = *self.seq.borrow() + 1;
        if let Value::Object(m) = &mut fields {
            m.insert("seq".into(), json!(n));
            m.insert("kind".into(), json!(kind));
            m.insert("ts_ms".into(), json!(now_ms()));
        }
        if b.len() >= CAP { b.pop_front(); }
        b.push_back((n, fields));
        let _ = self.seq.send(n);
    }

    pub fn last_seq(&self) -> u64 { *self.seq.borrow() }

    /// События с номером > `since`; если их нет — ждать до `wait`. Возвращает (последний номер, события).
    pub async fn wait(&self, since: u64, wait: Duration) -> (u64, Vec<Value>) {
        let mut rx = self.seq.subscribe();
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            {
                let b = self.buf.lock().await;
                let last = b.back().map(|(n, _)| *n).unwrap_or(0);
                // клиент «из будущего» (узел перезапущен) — отдать всё, что есть
                let since = if since > last { 0 } else { since };
                let ev: Vec<Value> = b.iter().filter(|(n, _)| *n > since).map(|(_, v)| v.clone()).collect();
                if !ev.is_empty() { return (last, ev); }
                if since == 0 && last == 0 && tokio::time::Instant::now() >= deadline { return (0, vec![]); }
            }
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => continue,
                _ => return (self.last_seq(), vec![]),
            }
        }
    }
}
