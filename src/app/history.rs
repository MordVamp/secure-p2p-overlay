//! app/history.rs — Локальная история результатов отправки и приёма (§13.1).
//!
//! Одна JSON-строка на событие в `<state_dir>/history.jsonl` (append-only,
//! права 0600: файл содержит тексты сообщений). Переживает перезапуск узла.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub ts_ms:      u64,
    /// "out" — отправка, "in" — приём.
    pub dir:        String,
    /// NodeID собеседника (hex).
    pub peer:       String,
    pub message_id: String,
    /// "delivered" (есть APP_ACK) | "sent" (без ожидания ACK) | "failed" | "received".
    pub status:     String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text:       Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms:     Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error:      Option<String>,
    /// Разбивка доставки (только для delivered): lookup, построение туннеля (None — переиспользован),
    /// проход кадра по цепочке до получателя.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookup_ms:  Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_build_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_send_ms:  Option<u64>,
    /// Групповой чат: id группы (hex) и её название.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group:      Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    /// Состав группы (NodeID hex) на момент сообщения — по нему клиенты восстанавливают группу.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members:    Option<Vec<String>>,
    /// Исходящее: получатель сообщил, что прочитал (отчёт о прочтении), unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_ms:    Option<u64>,
}

impl HistoryEntry {
    /// Строка-обновление статуса уже записанного сообщения (журнал только дописывается):
    /// `dir = "status"`; при чтении применяется к записи с тем же message_id и собеседником.
    pub fn status_update(peer: &str, message_id: &str, status: Option<&str>, read_ms: Option<u64>) -> Self {
        Self {
            ts_ms: crate::types::now_ms(), dir: "status".into(), peer: peer.into(), message_id: message_id.into(),
            status: status.unwrap_or("").into(), text: None, rtt_ms: None, error: None, lookup_ms: None,
            tunnel_build_ms: None, tunnel_send_ms: None, group: None, group_name: None, members: None, read_ms,
        }
    }
}

pub struct History {
    path: PathBuf,
    lock: Mutex<()>,
}

impl History {
    pub fn open(state_dir: &Path) -> Self {
        Self { path: state_dir.join("history.jsonl"), lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path { &self.path }

    /// Дописать событие. Ошибка записи не мешает доставке — только предупреждение.
    pub async fn append(&self, e: &HistoryEntry) {
        let _g = self.lock.lock().await;
        if let Err(err) = self.write_line(e) {
            warn!("history {}: {err}", self.path.display());
        }
    }

    fn write_line(&self, e: &HistoryEntry) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir)?; }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
        let mut f = opts.open(&self.path)?;
        let mut line = serde_json::to_vec(e)?;
        line.push(b'\n');
        f.write_all(&line)
    }

    /// Последние `limit` событий (старые → новые). Повреждённые строки пропускаются;
    /// строки-обновления (`dir = "status"`) применяются к своим записям и сами не возвращаются.
    pub async fn tail(&self, limit: usize) -> Vec<HistoryEntry> {
        let _g = self.lock.lock().await;
        let Ok(s) = std::fs::read_to_string(&self.path) else { return vec![] };
        let mut all: Vec<HistoryEntry> = Vec::new();
        let mut idx: std::collections::HashMap<(String, String), usize> = Default::default();
        for e in s.lines().filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok()) {
            if e.dir == "status" {
                // обновление только своей записи: тот же собеседник и message_id
                if let Some(&i) = idx.get(&(e.peer.clone(), e.message_id.clone())) {
                    let t = &mut all[i];
                    if !e.status.is_empty() { t.status = e.status; t.error = e.error.or(t.error.take()); }
                    if e.read_ms.is_some() { t.read_ms = e.read_ms; }
                    if e.rtt_ms.is_some() { t.rtt_ms = e.rtt_ms; }
                }
                continue;
            }
            idx.insert((e.peer.clone(), e.message_id.clone()), all.len());
            all.push(e);
        }
        let skip = all.len().saturating_sub(limit);
        all.into_iter().skip(skip).collect()
    }

    /// Есть ли уже входящее сообщение с таким id (дедупликация сообщений из почтового ящика DHT).
    pub async fn has_incoming(&self, message_id: &str) -> bool {
        let _g = self.lock.lock().await;
        let Ok(s) = std::fs::read_to_string(&self.path) else { return false };
        let needle = format!("\"message_id\":\"{message_id}\"");
        s.lines().any(|l| l.contains(&needle) && l.contains("\"dir\":\"in\""))
    }

    /// Собеседники, с которыми была переписка (для проверки почтовых ящиков).
    pub async fn peers(&self) -> Vec<String> {
        let _g = self.lock.lock().await;
        let Ok(s) = std::fs::read_to_string(&self.path) else { return vec![] };
        let mut v: Vec<String> = Vec::new();
        for e in s.lines().filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok()) {
            if (e.dir == "in" || e.dir == "out") && e.peer.len() == 64 && !v.contains(&e.peer) { v.push(e.peer); }
        }
        v
    }

    /// Удалить переписку с собеседником (или группу `g:<id>` и её входящие): файл переписывается.
    pub async fn delete_peer(&self, peer: &str) -> std::io::Result<usize> {
        let _g = self.lock.lock().await;
        let Ok(s) = std::fs::read_to_string(&self.path) else { return Ok(0) };
        let group = peer.strip_prefix("g:");
        let mut removed = 0;
        let mut out = String::with_capacity(s.len());
        for l in s.lines() {
            let drop = match serde_json::from_str::<HistoryEntry>(l) {
                Ok(e) => match group {
                    Some(g) => e.peer == peer || e.group.as_deref() == Some(g),
                    None => e.peer == peer && e.group.is_none(),
                },
                Err(_) => false,
            };
            if drop { removed += 1; } else { out.push_str(l); out.push('\n'); }
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, out)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?; }
        std::fs::rename(&tmp, &self.path)?;
        Ok(removed)
    }
}
