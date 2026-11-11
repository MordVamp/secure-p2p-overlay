//! app/mailbox.rs — офлайн-доставка через «почтовые ящики» в DHT.
//!
//! Сообщение, которое не удалось доставить через туннель (получатель выключен или недоступен
//! для входящих соединений — телефон за NAT), отправитель ставит в очередь и публикует
//! подписанной записью DHT под ключом `SHA-256("mbox:" ‖ получатель ‖ ":" ‖ отправитель)`.
//! Значение — список готовых E2E-конвертов: ретрансляторы и узлы-хранители видят только
//! шифротекст, адресованный получателю, и подпись отправителя на записи.
//!
//! Получатель периодически читает ящики известных собеседников (FIND_VALUE — исходящие
//! запросы, работают и за NAT), проверяет, что запись подписана именно этим собеседником,
//! расшифровывает, проверяет подпись сообщения и отвечает обычным APP_ACK. Отправитель,
//! получив APP_ACK, убирает сообщение из очереди и переопубликовывает ящик.
//!
//! Очередь отправителя сохраняется в `<state_dir>/outbox.json` и переживает перезапуск;
//! запись в DHT живёт `dht.ttl_seconds` и обновляется общим republish, пока отправитель в сети.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dht::storage::RecordKey;
use crate::types::{now_ms, NodeId};

/// Предел размера значения ящика (кадр DHT — до 64 КиБ вместе с подписью и заголовками).
pub const MAX_BODY_BYTES: usize = 40 * 1024;

/// Ключ ящика: письма от `sender` для `recipient`.
pub fn key(recipient: &NodeId, sender: &NodeId) -> RecordKey {
    RecordKey::from_str(&format!("mbox:{}:{}", recipient.to_hex(), sender.to_hex()))
}

/// Значение записи DHT.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MailboxBody {
    pub msgs: Vec<Vec<u8>>,
}

/// Сообщение в очереди отправителя.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queued {
    /// message_id (hex).
    pub id:       String,
    pub ts_ms:    u64,
    /// E2E-конверт (hex) — как он ушёл бы в туннель.
    pub envelope: String,
}

/// Очередь неотправленного: получатель (hex) → сообщения.
#[derive(Debug, Default)]
pub struct Outbox {
    path: Option<PathBuf>,
    q:    BTreeMap<String, Vec<Queued>>,
}

impl Outbox {
    pub fn in_memory() -> Self { Self::default() }

    pub fn load(state_dir: &Path) -> Self {
        let path = state_dir.join("outbox.json");
        let q = std::fs::read_to_string(&path).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path: Some(path), q }
    }

    fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Ok(s) = serde_json::to_string(&self.q) {
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                #[cfg(unix)]
                { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)); }
                let _ = std::fs::rename(&tmp, p);
            }
        }
    }

    /// Поставить сообщение в очередь к `to` (не больше `max` штук и `MAX_BODY_BYTES` — старые вытесняются).
    pub fn push(&mut self, to: &NodeId, item: Queued, max: usize) {
        let v = self.q.entry(to.to_hex()).or_default();
        v.retain(|x| x.id != item.id);
        v.push(item);
        while v.len() > max.max(1) || body_len(v) > MAX_BODY_BYTES && v.len() > 1 { v.remove(0); }
        self.save();
    }

    /// Убрать подтверждённое сообщение. true — было в очереди.
    pub fn remove(&mut self, to: &NodeId, id: &str) -> bool {
        let key = to.to_hex();
        let Some(v) = self.q.get_mut(&key) else { return false };
        let n = v.len();
        v.retain(|x| x.id != id);
        let removed = v.len() != n;
        if v.is_empty() { self.q.remove(&key); }
        if removed { self.save(); }
        removed
    }

    pub fn contains(&self, to: &NodeId, id: &str) -> bool {
        self.q.get(&to.to_hex()).map(|v| v.iter().any(|x| x.id == id)).unwrap_or(false)
    }

    /// Значение записи ящика для `to`.
    pub fn body(&self, to: &NodeId) -> MailboxBody {
        MailboxBody {
            msgs: self.q.get(&to.to_hex()).map(|v| v.iter().filter_map(|x| hex::decode(&x.envelope).ok()).collect())
                .unwrap_or_default(),
        }
    }

    /// Удалить сообщения старше `max_age_ms`; вернуть получателей, у которых ящик изменился,
    /// с id выброшенных сообщений.
    pub fn expire(&mut self, max_age_ms: u64) -> Vec<(String, Vec<String>)> {
        let now = now_ms();
        let mut changed = vec![];
        for (to, v) in self.q.iter_mut() {
            let old: Vec<String> = v.iter().filter(|x| x.ts_ms + max_age_ms < now).map(|x| x.id.clone()).collect();
            if !old.is_empty() {
                v.retain(|x| x.ts_ms + max_age_ms >= now);
                changed.push((to.clone(), old));
            }
        }
        self.q.retain(|_, v| !v.is_empty());
        if !changed.is_empty() { self.save(); }
        changed
    }

    /// Сколько сообщений ждёт по каждому получателю.
    pub fn summary(&self) -> BTreeMap<String, usize> {
        self.q.iter().map(|(k, v)| (k.clone(), v.len())).collect()
    }

    pub fn recipients(&self) -> Vec<String> { self.q.keys().cloned().collect() }
}

fn body_len(v: &[Queued]) -> usize { v.iter().map(|x| x.envelope.len() / 2 + 8).sum() }
