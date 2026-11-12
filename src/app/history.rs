//! app/history.rs — Локальная история результатов отправки и приёма (§13.1).
//!
//! Одна строка на событие в `<state_dir>/history.jsonl` (только дописывается, права 0600). Переживает перезапуск.
//!
//! **Шифрование на диске.** Если узлу дан ключ шифрования (`P2P_IDENTITY_KEK`: на Android — из Keystore, в desktop —
//! из хранилища секретов системы), каждая строка шифруется отдельно: `enc1:<id ключа>:<hex(nonce ‖ AES-256-GCM)>`.
//! Ключ данных — случайный, хранится в `history.keys` обёрнутым ключом, выведенным из KEK; его можно сменить
//! («перешифровать»: новый ключ, вся история переписывается, старый ключ стирается). Открытые строки (история до
//! включения шифрования) шифруются при первом запуске с ключом. Без KEK (узел из терминала) — открытый текст, как раньше.
//!
//! **Срок хранения** (`history.retention_days`, по умолчанию год): записи старше срока клиенты предлагают удалить,
//! перешифровать и оставить ещё на срок или вынести в открытый текстовый файл (опасно — помечается красным).

use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};
use zeroize::Zeroizing;

const LINE_AAD: &[u8] = b"p2p history v1";
const KEY_AAD: &[u8] = b"p2p history key v1";
const DAY_MS: u64 = 24 * 3600 * 1000;

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

/// Ключи данных истории (`history.keys`): обёрнутые ключом из KEK.
#[derive(Debug, Default, Serialize, Deserialize)]
struct KeyFile {
    /// id (8 hex) → hex(nonce ‖ обёрнутый ключ).
    keys:    Vec<(String, String)>,
    current: String,
    /// Решение «перешифровать и оставить»: до этого момента старые записи не предлагаются к удалению.
    #[serde(default)]
    snoozed_until_ms: u64,
}

struct Crypto {
    wrap:  Zeroizing<[u8; 32]>,
    keys:  Vec<(String, Zeroizing<[u8; 32]>)>,
    meta:  KeyFile,
}

pub struct History {
    path:      PathBuf,
    keys_path: PathBuf,
    export_dir: PathBuf,
    retention_ms: u64,
    lock:      Mutex<Option<Crypto>>,
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    {
        let mut o = std::fs::OpenOptions::new();
        o.create(true).write(true).truncate(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
        o.open(&tmp)?.write_all(data)?;
    }
    std::fs::rename(&tmp, path)
}

fn aead(key: &[u8; 32]) -> Aes256Gcm { Aes256Gcm::new_from_slice(key).expect("32 байта") }

fn seal(key: &[u8; 32], aad: &[u8], pt: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = aead(key).encrypt(Nonce::from_slice(&nonce), Payload { msg: pt, aad }).expect("AES-GCM");
    [nonce.as_slice(), &ct].concat()
}

fn open(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    if blob.len() < 12 + 16 { return None; }
    aead(key).decrypt(Nonce::from_slice(&blob[..12]), Payload { msg: &blob[12..], aad }).ok()
}

impl Crypto {
    fn new_key(&mut self) -> String {
        let mut k = Zeroizing::new([0u8; 32]);
        rand::rngs::OsRng.fill_bytes(k.as_mut());
        let mut idb = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut idb);
        let id = hex::encode(idb);
        let wrapped = seal(&self.wrap, &[KEY_AAD, id.as_bytes()].concat(), k.as_ref());
        self.meta.keys.push((id.clone(), hex::encode(wrapped)));
        self.meta.current = id.clone();
        self.keys.push((id.clone(), k));
        id
    }

    fn current(&self) -> Option<(&str, &[u8; 32])> {
        self.keys.iter().find(|(id, _)| *id == self.meta.current).map(|(id, k)| (id.as_str(), &**k))
    }

    fn encrypt_line(&self, json: &[u8]) -> Option<String> {
        let (id, k) = self.current()?;
        let ct = seal(k, &[LINE_AAD, id.as_bytes()].concat(), json);
        Some(format!("enc1:{id}:{}", hex::encode(ct)))
    }

    fn decrypt_line(&self, line: &str) -> Option<Vec<u8>> {
        let rest = line.strip_prefix("enc1:")?;
        let (id, hexct) = rest.split_once(':')?;
        let (_, k) = self.keys.iter().find(|(i, _)| i == id)?;
        open(k, &[LINE_AAD, id.as_bytes()].concat(), &hex::decode(hexct).ok()?)
    }
}

impl History {
    /// Открытая история без шифрования (узел без KEK, тесты).
    pub fn open(state_dir: &Path) -> Self { Self::open_with(state_dir, None, 365) }

    /// `data_key` — ключ из KEK (Some — история шифруется); `retention_days` — срок хранения.
    pub fn open_with(state_dir: &Path, data_key: Option<&[u8; 32]>, retention_days: u64) -> Self {
        let h = Self {
            path: state_dir.join("history.jsonl"), keys_path: state_dir.join("history.keys"),
            export_dir: state_dir.join("export"), retention_ms: retention_days.max(1) * DAY_MS,
            lock: Mutex::new(None),
        };
        if let Some(wrap) = data_key {
            match h.load_crypto(wrap) {
                Ok(c) => {
                    *h.lock.try_lock().expect("история только создана") = Some(c);
                    // строки, записанные до включения шифрования, — зашифровать
                    match h.migrate_plaintext() {
                        Ok(0) => {}
                        Ok(n) => info!("история: зашифровано {n} записей, сделанных до включения шифрования"),
                        Err(e) => warn!("история: шифрование старых записей не удалось: {e}"),
                    }
                }
                Err(e) => warn!("история: ключи не загружены ({e}) — история будет открытой"),
            }
        }
        h
    }

    fn load_crypto(&self, wrap: &[u8; 32]) -> std::io::Result<Crypto> {
        let mut c = Crypto { wrap: Zeroizing::new(*wrap), keys: vec![], meta: KeyFile::default() };
        if let Ok(s) = std::fs::read_to_string(&self.keys_path) {
            c.meta = serde_json::from_str(&s).map_err(std::io::Error::other)?;
            for (id, w) in &c.meta.keys {
                let blob = hex::decode(w).map_err(std::io::Error::other)?;
                let k = open(wrap, &[KEY_AAD, id.as_bytes()].concat(), &blob)
                    .ok_or_else(|| std::io::Error::other("ключ истории не расшифровывается: другой KEK?"))?;
                let k: [u8; 32] = k.as_slice().try_into().map_err(std::io::Error::other)?;
                c.keys.push((id.clone(), Zeroizing::new(k)));
            }
        }
        if c.current().is_none() {
            c.new_key();
            self.save_keys(&c)?;
        }
        Ok(c)
    }

    fn save_keys(&self, c: &Crypto) -> std::io::Result<()> {
        if let Some(d) = self.keys_path.parent() { std::fs::create_dir_all(d)?; }
        write_private(&self.keys_path, &serde_json::to_vec_pretty(&c.meta)?)
    }

    /// Открытые строки → зашифрованные (файл переписывается). Число зашифрованных.
    fn migrate_plaintext(&self) -> std::io::Result<usize> {
        let g = self.lock.try_lock().map_err(std::io::Error::other)?;
        let Some(c) = g.as_ref() else { return Ok(0) };
        let Ok(s) = std::fs::read_to_string(&self.path) else { return Ok(0) };
        let plain = s.lines().filter(|l| !l.starts_with("enc1:") && !l.trim().is_empty()).count();
        if plain == 0 { return Ok(0); }
        let mut out = String::with_capacity(s.len() * 2);
        for l in s.lines().filter(|l| !l.trim().is_empty()) {
            if l.starts_with("enc1:") { out.push_str(l); } else { out.push_str(&c.encrypt_line(l.as_bytes()).unwrap_or_else(|| l.into())); }
            out.push('\n');
        }
        write_private(&self.path, out.as_bytes())?;
        Ok(plain)
    }

    pub fn path(&self) -> &Path { &self.path }

    /// Шифруется ли история на диске.
    pub async fn encrypted(&self) -> bool { self.lock.lock().await.is_some() }

    /// Дописать событие. Ошибка записи не мешает доставке — только предупреждение.
    pub async fn append(&self, e: &HistoryEntry) {
        let g = self.lock.lock().await;
        if let Err(err) = self.write_line(g.as_ref(), e) {
            warn!("history {}: {err}", self.path.display());
        }
    }

    fn write_line(&self, c: Option<&Crypto>, e: &HistoryEntry) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir)?; }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
        let mut f = opts.open(&self.path)?;
        let json = serde_json::to_vec(e)?;
        let mut line = match c.and_then(|c| c.encrypt_line(&json)) { Some(l) => l.into_bytes(), None => json };
        line.push(b'\n');
        f.write_all(&line)
    }

    /// Все записи (сырые, вместе со строками-обновлениями), расшифрованные.
    fn read_all(&self, c: Option<&Crypto>) -> Vec<HistoryEntry> {
        let Ok(s) = std::fs::read_to_string(&self.path) else { return vec![] };
        let mut bad = 0;
        let out: Vec<HistoryEntry> = s.lines().filter_map(|l| {
            let e = if l.starts_with("enc1:") {
                c.and_then(|c| c.decrypt_line(l)).and_then(|j| serde_json::from_slice::<HistoryEntry>(&j).ok())
            } else { serde_json::from_str::<HistoryEntry>(l).ok() };
            if e.is_none() && !l.trim().is_empty() { bad += 1; }
            e
        }).collect();
        if bad > 0 { tracing::debug!("история: {bad} строк не прочитано (повреждены или нет ключа)"); }
        out
    }

    /// Переписать историю целиком (текущим ключом, если шифрование включено).
    fn write_all(&self, c: Option<&Crypto>, entries: &[HistoryEntry]) -> std::io::Result<()> {
        let mut out = String::new();
        for e in entries {
            let json = serde_json::to_string(e)?;
            out.push_str(&match c.and_then(|c| c.encrypt_line(json.as_bytes())) { Some(l) => l, None => json });
            out.push('\n');
        }
        write_private(&self.path, out.as_bytes())
    }

    /// Последние `limit` событий (старые → новые). Повреждённые строки пропускаются;
    /// строки-обновления (`dir = "status"`) применяются к своим записям и сами не возвращаются.
    pub async fn tail(&self, limit: usize) -> Vec<HistoryEntry> {
        let g = self.lock.lock().await;
        let mut all: Vec<HistoryEntry> = Vec::new();
        let mut idx: std::collections::HashMap<(String, String), usize> = Default::default();
        for e in self.read_all(g.as_ref()) {
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
        let g = self.lock.lock().await;
        self.read_all(g.as_ref()).iter().any(|e| e.dir == "in" && e.message_id == message_id)
    }

    /// Писали ли мы сами этому узлу (признак «знакомого» для автоматического приёма файлов).
    pub async fn has_outgoing_to(&self, peer: &str) -> bool {
        let g = self.lock.lock().await;
        self.read_all(g.as_ref()).iter().any(|e| e.dir == "out" && e.peer == peer)
    }

    /// Собеседники, с которыми была переписка (для проверки почтовых ящиков).
    pub async fn peers(&self) -> Vec<String> {
        let g = self.lock.lock().await;
        let mut v: Vec<String> = Vec::new();
        for e in self.read_all(g.as_ref()) {
            if (e.dir == "in" || e.dir == "out") && e.peer.len() == 64 && !v.contains(&e.peer) { v.push(e.peer); }
        }
        v
    }

    /// Удалить переписку с собеседником (или группу `g:<id>` и её входящие): файл переписывается.
    pub async fn delete_peer(&self, peer: &str) -> std::io::Result<usize> {
        let g = self.lock.lock().await;
        let group = peer.strip_prefix("g:");
        let all = self.read_all(g.as_ref());
        let before = all.len();
        let keep: Vec<HistoryEntry> = all.into_iter().filter(|e| !match group {
            Some(gr) => e.peer == peer || e.group.as_deref() == Some(gr),
            None => e.peer == peer && e.group.is_none(),
        }).collect();
        let removed = before - keep.len();
        if removed > 0 { self.write_all(g.as_ref(), &keep)?; }
        Ok(removed)
    }

    // ── срок хранения ───────────────────────────────────────────────────────

    /// Состояние: шифрование, срок, сколько записей старше срока, нужно ли спросить пользователя.
    pub async fn retention(&self, now_ms: u64) -> serde_json::Value {
        let g = self.lock.lock().await;
        let all = self.read_all(g.as_ref());
        let cutoff = now_ms.saturating_sub(self.retention_ms);
        let expired = all.iter().filter(|e| e.ts_ms < cutoff && e.dir != "status").count();
        let snoozed = g.as_ref().map_or(0, |c| c.meta.snoozed_until_ms);
        serde_json::json!({
            "encrypted": g.is_some(), "key_id": g.as_ref().map(|c| c.meta.current.clone()),
            "retention_days": self.retention_ms / DAY_MS, "records": all.iter().filter(|e| e.dir != "status").count(),
            "expired": expired, "oldest_ms": all.iter().map(|e| e.ts_ms).min(),
            "snoozed_until_ms": snoozed, "due": expired > 0 && now_ms >= snoozed,
        })
    }

    /// Удалить записи старше срока. Сколько удалено.
    pub async fn delete_expired(&self, now_ms: u64) -> std::io::Result<usize> {
        let g = self.lock.lock().await;
        let cutoff = now_ms.saturating_sub(self.retention_ms);
        let all = self.read_all(g.as_ref());
        let keep: Vec<HistoryEntry> = all.iter().filter(|e| e.ts_ms >= cutoff).cloned().collect();
        let n = all.len() - keep.len();
        if n > 0 { self.write_all(g.as_ref(), &keep)?; }
        Ok(n)
    }

    /// Перешифровать: новый ключ данных, вся история переписывается им, старые ключи стираются.
    /// `keep_expired` — пользователь решил оставить старые записи ещё на срок (вопрос не повторяется до тех пор).
    pub async fn reencrypt(&self, now_ms: u64, keep_expired: bool) -> std::io::Result<serde_json::Value> {
        let mut g = self.lock.lock().await;
        let Some(c) = g.as_mut() else { return Err(std::io::Error::other("шифрование истории не включено (узел запущен без ключа)")) };
        let all = self.read_all(Some(c));
        let old = c.meta.current.clone();
        c.new_key();
        let id = c.meta.current.clone();
        c.keys.retain(|(i, _)| *i == id);
        c.meta.keys.retain(|(i, _)| *i == id);
        if keep_expired { c.meta.snoozed_until_ms = now_ms + self.retention_ms; }
        // сначала история новым ключом, потом файл ключей: при сбое между ними старый ключ ещё на месте
        self.write_all(Some(c), &all)?;
        self.save_keys(c)?;
        info!("история перешифрована: ключ {old} → {id}, записей {}", all.len());
        Ok(serde_json::json!({ "key_id": id, "records": all.len(), "snoozed_until_ms": c.meta.snoozed_until_ms }))
    }

    /// Вынести записи старше срока в открытый текстовый файл `<state>/export/` и удалить их из истории.
    /// Опасно: файл не зашифрован. Возвращает путь и число записей.
    pub async fn export_expired(&self, now_ms: u64) -> std::io::Result<(PathBuf, usize)> {
        let g = self.lock.lock().await;
        let cutoff = now_ms.saturating_sub(self.retention_ms);
        let all = self.read_all(g.as_ref());
        let (old, keep): (Vec<HistoryEntry>, Vec<HistoryEntry>) = all.into_iter().partition(|e| e.ts_ms < cutoff);
        let msgs: Vec<&HistoryEntry> = old.iter().filter(|e| e.dir != "status").collect();
        std::fs::create_dir_all(&self.export_dir)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&self.export_dir, std::fs::Permissions::from_mode(0o700)); }
        let stamp = chrono::DateTime::from_timestamp_millis(now_ms as i64).unwrap_or_default().format("%Y%m%d-%H%M%S");
        let path = self.export_dir.join(format!("history-{stamp}.txt"));
        let mut text = String::from("# ВНИМАНИЕ: файл НЕ ЗАШИФРОВАН — переписка в открытом виде. Храните его сами или удалите.\n\
                                     # Время — UTC; → исходящее, ← входящее; собеседник — NodeID.\n\n");
        for e in &msgs {
            let t = chrono::DateTime::from_timestamp_millis(e.ts_ms as i64).unwrap_or_default().format("%Y-%m-%d %H:%M");
            let arrow = if e.dir == "out" { "→" } else { "←" };
            let grp = e.group_name.as_deref().map(|g| format!(" [группа «{g}»]")).unwrap_or_default();
            text.push_str(&format!("{t}  {arrow} {}{grp}  {}\n", e.peer, e.text.as_deref().unwrap_or("")));
        }
        write_private(&path, text.as_bytes())?;
        self.write_all(g.as_ref(), &keep)?;
        warn!("история: {} записей вынесены в ОТКРЫТЫЙ файл {}", msgs.len(), path.display());
        Ok((path, msgs.len()))
    }
}
