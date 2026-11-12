//! app/files.rs — передача файлов (ТЗ §13.2) поверх того же E2E-канала, что и сообщения.
//!
//! Протокол (каждое сообщение — подписанный и зашифрованный для получателя `AppMessage`):
//! ```text
//! A → B  FileOffer {transfer_id, name, size, sha256, block_size, blocks}
//! B → A  FileAck   {transfer_id, received: 0, missing: []}  — согласие (сразу или после решения пользователя)
//!        или FileDone {ok: false, error}                    — отказ (квота, пользователь, тайм-аут предложения)
//! A → B  FileBlock {transfer_id, index, data}        × окно (WINDOW блоков)
//! B → A  FileAck   {transfer_id, received, missing}  после окна или последнего блока
//!        … A повторяет пропущенные блоки, затем следующее окно …
//! B → A  FileDone  {transfer_id, ok, error}          после проверки SHA-256 всего файла
//! ```
//! Приём: только в `<state_dir>/downloads/<NodeID отправителя, 16 hex>/` (файл больше 2 ГиБ — в подпапку
//! «Большие файлы»; неполный файл — `downloads/.partial/<id>.part`), безопасное имя,
//! ограничение размера, дубликаты блоков игнорируются, блоки от чужого узла отвергаются, итоговая проверка
//! SHA-256 (несовпадение → файл удаляется), неполный файл удаляется по тайм-ауту. Возобновление прерванной
//! передачи не реализовано: после тайм-аута передача начинается заново.
//!
//! Защита получателя (без неё любой узел мог бы залить на диск сколько угодно файлов по 64 МиБ): согласие
//! (`files.accept`), не больше 2 одновременных приёмов от узла и 8 всего, не больше 3 / 16 ждущих решения
//! предложений, все принятые файлы вместе — не больше `max_total_bytes`, на диске остаётся `min_free_bytes`.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::{now_ms, NodeId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOffer {
    pub transfer_id: Vec<u8>,
    pub name:        String,
    pub size:        u64,
    pub sha256:      Vec<u8>,
    pub block_size:  u32,
    pub blocks:      u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBlock { pub transfer_id: Vec<u8>, pub index: u32, pub data: Vec<u8> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAck { pub transfer_id: Vec<u8>, pub received: u32, pub missing: Vec<u32> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDone { pub transfer_id: Vec<u8>, pub ok: bool, pub error: Option<String> }

/// Имя без путей и служебных символов: `../x`, `a/b`, `.hidden`, пустое и слишком длинное отвергаются.
pub fn safe_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() { bail!("пустое имя файла"); }
    if n.len() > 200 { bail!("имя файла длиннее 200 байт"); }
    if n.starts_with('.') { bail!("имя файла не может начинаться с точки"); }
    if n.contains("..") { bail!("имя файла не может содержать '..'"); }
    if n.chars().any(|c| c == '/' || c == '\\' || c == ':' || c.is_control()) {
        bail!("имя файла содержит недопустимые символы (/ \\ : или управляющие)");
    }
    Ok(n.to_owned())
}

pub fn sha256_file(path: &Path) -> Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_vec())
}

/// Результат приёма блока.
#[derive(Debug, PartialEq)]
pub enum BlockOutcome { Stored, Duplicate, Complete }

struct Incoming {
    from:       NodeId,
    offer:      FileOffer,
    have:       Vec<bool>,
    received:   u32,
    duplicates: u32,
    part:       PathBuf,
    last_ms:    u64,
}

/// Сводка передачи для API и интерфейса.
#[derive(Debug, Clone, Serialize)]
pub struct TransferInfo {
    pub transfer_id: String,
    pub dir:         String,
    pub peer:        String,
    pub name:        String,
    pub size:        u64,
    pub blocks:      u32,
    pub done_blocks: u32,
    pub status:      String,
    pub error:       Option<String>,
    pub path:        Option<String>,
    pub started_ms:  u64,
    pub finished_ms: Option<u64>,
    /// Почему нужно согласие / предупреждение (незнакомый узел, большой или очень большой файл, обманчивое имя).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning:     Option<String>,
    /// Настоящее расширение (по концу имени на диске) — клиенты показывают его отдельно от имени.
    #[serde(default)]
    pub ext:         String,
}

/// Файл больше этого — «очень большой»: отдельное предупреждение и подпапка `LARGE_DIR`.
pub const LARGE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const LARGE_DIR: &str = "Большие файлы";

/// Каталог — только владельцу (0700): принятые файлы не читают другие пользователи компьютера.
/// На Windows не действует: там доступ задают ACL, и каталог в профиле пользователя и так закрыт от других.
fn private_dir(d: &Path) {
    let _ = std::fs::create_dir_all(d);
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)); }
}

/// Папка собеседника в `downloads/`: первые 16 hex его NodeID (по ней клиенты показывают имя из контактов).
pub fn peer_dir(peer: &NodeId) -> String { peer.to_hex()[..16].to_owned() }

/// Размер для людей: 950 КиБ, 12,5 МиБ, 3,1 ГиБ.
pub fn human_bytes(n: u64) -> String {
    let f = n as f64;
    if f >= 1024.0 * 1024.0 * 1024.0 { format!("{:.1} ГиБ", f / (1024.0 * 1024.0 * 1024.0)).replace('.', ",") }
    else if f >= 1024.0 * 1024.0 { format!("{:.1} МиБ", f / (1024.0 * 1024.0)).replace('.', ",") }
    else { format!("{:.0} КиБ", (f / 1024.0).max(1.0)) }
}

// ── имя файла: настоящее расширение и скрытые символы ────────────────────────

/// Невидимые символы и символы управления направлением текста (Unicode Cf и разделители строк). Законны (арабский,
/// иврит, эмодзи, мягкий перенос), поэтому не отвергаются; опасны в сочетании с исполняемым расширением:
/// «счёт_⟨U+202E⟩fdp.exe» на экране выглядит как «счёт_exe.pdf».
pub fn is_hidden_char(c: char) -> bool {
    matches!(c as u32, 0x00AD | 0x061C | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xFEFF)
}

/// Настоящее расширение — то, что стоит в конце имени на диске (а не то, что видно после переворота текста).
pub fn file_ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 12 =>
            ext.chars().filter(|c| !is_hidden_char(*c)).collect::<String>().to_lowercase(),
        _ => String::new(),
    }
}

/// Расширения, которые запускаются при открытии (Windows, Linux, macOS, Android, Java, скрипты, ярлыки).
pub const EXECUTABLE_EXT: &[&str] = &[
    "exe", "com", "bat", "cmd", "scr", "pif", "msi", "msp", "lnk", "cpl", "hta", "reg", "vbs", "vbe", "js", "jse", "wsf",
    "ps1", "psm1", "sh", "bash", "run", "bin", "desktop", "appimage", "deb", "rpm", "app", "command", "pkg", "dmg",
    "apk", "xapk", "jar", "py", "pl",
];

/// Имя для показа: скрытые символы — видимыми обозначениями ⟨U+202E⟩.
pub fn visible_name(name: &str) -> String {
    name.chars().map(|c| if is_hidden_char(c) { format!("⟨U+{:04X}⟩", c as u32) } else { c.to_string() }).collect()
}

/// Предупреждение об имени: скрытые символы в имени исполняемого файла.
pub fn name_warning(name: &str) -> Option<String> {
    let ext = file_ext(name);
    if !name.chars().any(is_hidden_char) || !EXECUTABLE_EXT.contains(&ext.as_str()) { return None; }
    Some(format!("⚠ в имени исполняемого файла (.{ext}) есть невидимые символы или символы смены направления текста — \
                  на экране имя может выглядеть иначе; на самом деле: «{}»", visible_name(name)))
}

/// Принятые файлы на диске по папкам собеседников (переживает перезапуск — в отличие от списка передач):
/// `[{folder, peer_prefix, files: [{name, size, mtime_ms, large, path}]}]`, файлы новее — выше.
pub fn list_downloads(dir: &Path) -> serde_json::Value {
    fn files_in(d: &Path, large: bool) -> Vec<serde_json::Value> {
        let Ok(rd) = std::fs::read_dir(d) else { return vec![] };
        rd.filter_map(|e| e.ok()).filter_map(|e| {
            let m = e.metadata().ok()?;
            if !m.is_file() { return None; }
            let mt = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
            let name = e.file_name().to_string_lossy().into_owned();
            Some(serde_json::json!({ "ext": file_ext(&name), "name_warning": name_warning(&name), "name": name,
                                     "size": m.len(), "mtime_ms": mt, "large": large, "path": e.path().display().to_string() }))
        }).collect()
    }
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !e.path().is_dir() || name.starts_with('.') { continue; }
            let mut files = files_in(&e.path(), false);
            files.extend(files_in(&e.path().join(LARGE_DIR), true));
            files.sort_by_key(|f| std::cmp::Reverse(f["mtime_ms"].as_u64().unwrap_or(0)));
            if !files.is_empty() { out.push(serde_json::json!({ "folder": name, "peer_prefix": name, "files": files })); }
        }
    }
    // файлы, принятые до разделения по папкам, — отдельной группой
    let old = files_in(dir, false);
    if !old.is_empty() { out.push(serde_json::json!({ "folder": "", "peer_prefix": "", "files": old })); }
    serde_json::Value::Array(out)
}

/// Пределы приёма.
pub const MAX_INCOMING_PER_PEER: usize = 2;
pub const MAX_INCOMING_TOTAL: usize = 8;
pub const MAX_PENDING_PER_PEER: usize = 3;
pub const MAX_PENDING_TOTAL: usize = 16;

/// Настройки приёма, меняемые пользователем в приложении (`<state_dir>/file_settings.json` поверх `files.*`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePolicy {
    /// known | ask | auto
    pub accept:            String,
    pub auto_accept_bytes: u64,
    pub max_total_bytes:   u64,
}

impl FilePolicy {
    pub fn path(state_dir: &Path) -> PathBuf { state_dir.join("file_settings.json") }

    /// Сохранённые настройки или значения из конфигурации.
    pub fn load(state_dir: &Path, default: FilePolicy) -> FilePolicy {
        std::fs::read_to_string(Self::path(state_dir)).ok()
            .and_then(|s| serde_json::from_str::<FilePolicy>(&s).ok())
            .filter(|p| ["known", "ask", "auto"].contains(&p.accept.as_str()))
            .unwrap_or(default)
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        let p = Self::path(state_dir);
        std::fs::write(&p, serde_json::to_vec_pretty(self)?)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)); }
        Ok(())
    }
}

/// Доля квоты, после которой клиенты предупреждают «очистите или увеличьте».
pub const QUOTA_WARN_PCT: f64 = 90.0;

/// Квоты места (из `files.*`).
#[derive(Debug, Clone, Copy)]
pub struct Quota { pub max_total_bytes: u64, pub min_free_bytes: u64 }

impl Default for Quota {
    fn default() -> Self { Self { max_total_bytes: u64::MAX, min_free_bytes: 0 } }
}

/// Приёмник файлов одного узла.
pub struct FileStore {
    pub dir:        PathBuf,
    pub max_bytes:  u64,
    pub timeout_ms: u64,
    pub quota:      Quota,
    incoming:       HashMap<Vec<u8>, Incoming>,
    /// Предложения, ждущие решения пользователя: transfer_id → (от кого, предложение, когда пришло).
    pending:        HashMap<Vec<u8>, (NodeId, FileOffer, u64)>,
}

/// Свободное место на разделе с `dir` (None — не удалось узнать).
pub fn free_bytes(dir: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut p = dir.to_path_buf();
        while !p.exists() { if !p.pop() { return None; } }
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c — корректная C-строка, st — выходной буфер нужного типа
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 { return None; }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
    #[cfg(not(unix))]
    { let _ = dir; None }
}

impl FileStore {
    pub fn new(state_dir: &Path, max_bytes: u64, timeout_ms: u64) -> Self {
        Self { dir: state_dir.join("downloads"), max_bytes, timeout_ms, quota: Quota::default(),
               incoming: HashMap::new(), pending: HashMap::new() }
    }

    pub fn with_quota(self, quota: Quota) -> Self { Self { quota, ..self } }

    fn part_dir(&self) -> PathBuf { self.dir.join(".partial") }

    /// Куда ляжет принятый файл: папка собеседника, для очень больших — подпапка «Большие файлы».
    pub fn target_dir(&self, from: &NodeId, size: u64) -> PathBuf {
        let d = self.dir.join(peer_dir(from));
        if size > LARGE_FILE_BYTES { d.join(LARGE_DIR) } else { d }
    }

    /// Сколько занимают уже принятые файлы (`downloads/` с папками собеседников, без `.partial`).
    /// Занято принятыми файлами и квота (для предупреждения при 90 %).
    pub fn usage(&self) -> (u64, u64) { (self.used_bytes(), self.quota.max_total_bytes) }

    /// Удалить принятый файл — только внутри `downloads/` (не `.partial`). Ok — сколько байт освобождено.
    pub fn delete_received(&self, path: &Path) -> Result<u64> {
        let root = self.dir.canonicalize().context("downloads/ нет")?;
        let p = path.canonicalize().context("файл не найден")?;
        if !p.starts_with(&root) || p.starts_with(root.join(".partial")) || !p.is_file() {
            bail!("можно удалять только принятые файлы в {}", root.display());
        }
        let n = std::fs::metadata(&p)?.len();
        std::fs::remove_file(&p)?;
        // пустая папка собеседника (и «Большие файлы») — тоже
        let mut d = p.parent().map(Path::to_path_buf);
        while let Some(dir) = d {
            if dir == root || std::fs::remove_dir(&dir).is_err() { break; }
            d = dir.parent().map(Path::to_path_buf);
        }
        Ok(n)
    }

    fn used_bytes(&self) -> u64 {
        fn walk(d: &Path, depth: u32) -> u64 {
            let Ok(rd) = std::fs::read_dir(d) else { return 0 };
            rd.filter_map(|e| e.ok()).map(|e| {
                let Ok(m) = e.metadata() else { return 0 };
                if m.is_file() { m.len() }
                else if m.is_dir() && depth < 3 && e.file_name() != ".partial" { walk(&e.path(), depth + 1) }
                else { 0 }
            }).sum()
        }
        walk(&self.dir, 0)
    }

    /// Проверить предложение: формат, имя, размер, пределы одновременных приёмов и квоты места.
    /// `by_user` — пользователь сам принял файл: общая квота принятых файлов не действует (только свободное место).
    pub fn check_offer(&self, from: NodeId, o: &FileOffer, by_user: bool) -> Result<()> {
        if o.transfer_id.len() != 16 { bail!("transfer_id должен быть 16 байт"); }
        if self.incoming.contains_key(&o.transfer_id) || self.pending.contains_key(&o.transfer_id) {
            bail!("передача с таким transfer_id уже идёт");
        }
        safe_name(&o.name)?;
        if o.size > self.max_bytes { bail!("файл {} байт больше лимита {} байт", o.size, self.max_bytes); }
        if o.sha256.len() != 32 { bail!("sha256 должен быть 32 байта"); }
        if o.block_size == 0 || o.block_size as usize > MAX_BLOCK { bail!("недопустимый размер блока {}", o.block_size); }
        let expect = o.size.div_ceil(o.block_size as u64).max(1);
        if o.blocks as u64 != expect { bail!("blocks = {}, а для размера {} нужно {expect}", o.blocks, o.size); }
        if self.incoming.values().filter(|i| i.from == from).count() >= MAX_INCOMING_PER_PEER {
            bail!("от этого узла уже идут {MAX_INCOMING_PER_PEER} приёма — подождите");
        }
        if self.incoming.len() >= MAX_INCOMING_TOTAL { bail!("получатель уже принимает {MAX_INCOMING_TOTAL} файлов — подождите"); }
        let reserved: u64 = self.incoming.values().map(|i| i.offer.size).sum();
        if !by_user && self.used_bytes() + reserved + o.size > self.quota.max_total_bytes {
            bail!("у получателя исчерпана квота на принятые файлы ({} МиБ)", self.quota.max_total_bytes >> 20);
        }
        if let Some(free) = free_bytes(&self.dir) {
            if free < o.size.saturating_add(reserved).saturating_add(self.quota.min_free_bytes) {
                bail!("у получателя мало места на диске");
            }
        }
        Ok(())
    }

    /// Отложить предложение до решения пользователя (с пределами на число ждущих).
    pub fn hold(&mut self, from: NodeId, o: FileOffer) -> Result<()> {
        if self.pending.values().filter(|(f, _, _)| *f == from).count() >= MAX_PENDING_PER_PEER {
            bail!("от этого узла уже {MAX_PENDING_PER_PEER} предложения ждут решения");
        }
        if self.pending.len() >= MAX_PENDING_TOTAL { bail!("слишком много предложений ждут решения"); }
        self.pending.insert(o.transfer_id.clone(), (from, o, now_ms()));
        Ok(())
    }

    /// Пользователь решил: забрать ждущее предложение.
    pub fn take_pending(&mut self, id: &[u8]) -> Option<(NodeId, FileOffer)> {
        self.pending.remove(id).map(|(f, o, _)| (f, o))
    }

    /// Предложения без ответа дольше `timeout_ms` — убрать и вернуть (им уйдёт отказ).
    pub fn expire_pending(&mut self, timeout_ms: u64) -> Vec<(NodeId, Vec<u8>)> {
        let now = now_ms();
        let old: Vec<Vec<u8>> = self.pending.iter().filter(|(_, (_, _, t))| now.saturating_sub(*t) > timeout_ms)
            .map(|(k, _)| k.clone()).collect();
        old.into_iter().filter_map(|k| self.pending.remove(&k).map(|(f, _, _)| (f, k))).collect()
    }

    /// Принять предложение: проверить имя, размер, квоты; создать неполный файл.
    pub fn on_offer(&mut self, from: NodeId, o: FileOffer, by_user: bool) -> Result<()> {
        self.check_offer(from, &o, by_user)?;
        std::fs::create_dir_all(self.part_dir())?;
        let part = self.part_dir().join(format!("{}.part", hex::encode(&o.transfer_id)));
        private_dir(&self.dir);
        private_dir(&self.part_dir());
        let mut oo = std::fs::OpenOptions::new();
        oo.create(true).write(true).truncate(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; oo.mode(0o600); }   // принятый файл — только владельцу
        let f = oo.open(&part).with_context(|| format!("{}", part.display()))?;
        f.set_len(o.size)?;
        self.incoming.insert(o.transfer_id.clone(), Incoming {
            from, have: vec![false; o.blocks as usize], received: 0, duplicates: 0, part, last_ms: now_ms(), offer: o,
        });
        Ok(())
    }

    /// Записать блок. Чужой отправитель, неизвестная передача, неверный индекс или длина — ошибка.
    pub fn on_block(&mut self, from: NodeId, b: &FileBlock) -> Result<BlockOutcome> {
        let inc = self.incoming.get_mut(&b.transfer_id).context("блок неизвестной передачи")?;
        if inc.from != from { bail!("блок от другого узла {}", from.short()); }
        let o = &inc.offer;
        if b.index >= o.blocks { bail!("индекс блока {} вне 0..{}", b.index, o.blocks); }
        let off = b.index as u64 * o.block_size as u64;
        let want = (o.size - off.min(o.size)).min(o.block_size as u64) as usize;
        if b.data.len() != want { bail!("блок {}: {} байт вместо {want}", b.index, b.data.len()); }
        inc.last_ms = now_ms();
        if inc.have[b.index as usize] { inc.duplicates += 1; return Ok(BlockOutcome::Duplicate); }
        let mut f = std::fs::OpenOptions::new().write(true).open(&inc.part)?;
        f.seek(SeekFrom::Start(off))?;
        f.write_all(&b.data)?;
        inc.have[b.index as usize] = true;
        inc.received += 1;
        Ok(if inc.received == o.blocks { BlockOutcome::Complete } else { BlockOutcome::Stored })
    }

    /// Подтверждение: сколько блоков есть и какие из первых `upto` отсутствуют.
    pub fn ack(&self, id: &[u8], upto: u32) -> Option<FileAck> {
        let inc = self.incoming.get(id)?;
        let missing = (0..upto.min(inc.offer.blocks)).filter(|&i| !inc.have[i as usize]).collect();
        Some(FileAck { transfer_id: id.to_vec(), received: inc.received, missing })
    }

    /// Все блоки получены: проверить SHA-256 и переместить в downloads/. При несовпадении файл удаляется.
    pub fn finish(&mut self, id: &[u8]) -> Result<PathBuf> {
        let inc = self.incoming.remove(id).context("неизвестная передача")?;
        let digest = sha256_file(&inc.part)?;
        if digest != inc.offer.sha256 {
            let _ = std::fs::remove_file(&inc.part);
            bail!("SHA-256 не совпадает — файл отброшен");
        }
        let name = safe_name(&inc.offer.name)?;
        let dir = self.target_dir(&inc.from, inc.offer.size);
        std::fs::create_dir_all(&dir)?;
        private_dir(&self.dir.join(peer_dir(&inc.from)));
        private_dir(&dir);
        let mut dst = dir.join(&name);
        let (stem, ext) = match name.rsplit_once('.') { Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")), _ => (name.clone(), String::new()) };
        let mut k = 1;
        while dst.exists() { dst = dir.join(format!("{stem} ({k}){ext}")); k += 1; }
        std::fs::rename(&inc.part, &dst)?;
        Ok(dst)
    }

    /// Отменить приём (ошибка у отправителя или тайм-аут): удалить неполный файл.
    pub fn abort(&mut self, id: &[u8]) {
        if let Some(inc) = self.incoming.remove(id) { let _ = std::fs::remove_file(&inc.part); }
    }

    /// Удалить неполные файлы, по которым не было блоков дольше тайм-аута. Возвращает их transfer_id.
    pub fn evict_stale(&mut self) -> Vec<Vec<u8>> {
        let now = now_ms();
        let stale: Vec<Vec<u8>> = self.incoming.iter()
            .filter(|(_, i)| now.saturating_sub(i.last_ms) > self.timeout_ms).map(|(k, _)| k.clone()).collect();
        for id in &stale { self.abort(id); }
        stale
    }

    pub fn progress(&self, id: &[u8]) -> Option<(u32, u32, u32)> {
        self.incoming.get(id).map(|i| (i.received, i.offer.blocks, i.duplicates))
    }

    pub fn sender_of(&self, id: &[u8]) -> Option<NodeId> { self.incoming.get(id).map(|i| i.from) }
}

/// Максимальный блок: AppMessage + подпись + конверт должны уложиться в сегмент туннеля 48 КиБ.
pub const MAX_BLOCK: usize = 44 * 1024;
