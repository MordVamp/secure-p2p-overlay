//! app/files.rs — передача файлов (ТЗ §13.2) поверх того же E2E-канала, что и сообщения.
//!
//! Протокол (каждое сообщение — подписанный и зашифрованный для получателя `AppMessage`):
//! ```text
//! A → B  FileOffer {transfer_id, name, size, sha256, block_size, blocks}
//! A → B  FileBlock {transfer_id, index, data}        × окно (WINDOW блоков)
//! B → A  FileAck   {transfer_id, received, missing}  после окна или последнего блока
//!        … A повторяет пропущенные блоки, затем следующее окно …
//! B → A  FileDone  {transfer_id, ok, error}          после проверки SHA-256 всего файла
//! ```
//! Приём: только в `<state_dir>/downloads/` (неполный файл — `downloads/.partial/<id>.part`), безопасное имя,
//! ограничение размера, дубликаты блоков игнорируются, блоки от чужого узла отвергаются, итоговая проверка
//! SHA-256 (несовпадение → файл удаляется), неполный файл удаляется по тайм-ауту. Возобновление прерванной
//! передачи не реализовано: после тайм-аута передача начинается заново.

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
}

/// Приёмник файлов одного узла.
pub struct FileStore {
    pub dir:        PathBuf,
    pub max_bytes:  u64,
    pub timeout_ms: u64,
    incoming:       HashMap<Vec<u8>, Incoming>,
}

impl FileStore {
    pub fn new(state_dir: &Path, max_bytes: u64, timeout_ms: u64) -> Self {
        Self { dir: state_dir.join("downloads"), max_bytes, timeout_ms, incoming: HashMap::new() }
    }

    fn part_dir(&self) -> PathBuf { self.dir.join(".partial") }

    /// Принять предложение: проверить имя, размер, согласованность числа блоков; создать неполный файл.
    pub fn on_offer(&mut self, from: NodeId, o: FileOffer) -> Result<()> {
        if o.transfer_id.len() != 16 { bail!("transfer_id должен быть 16 байт"); }
        if self.incoming.contains_key(&o.transfer_id) { bail!("передача с таким transfer_id уже идёт"); }
        safe_name(&o.name)?;
        if o.size > self.max_bytes { bail!("файл {} байт больше лимита {} байт", o.size, self.max_bytes); }
        if o.sha256.len() != 32 { bail!("sha256 должен быть 32 байта"); }
        if o.block_size == 0 || o.block_size as usize > MAX_BLOCK { bail!("недопустимый размер блока {}", o.block_size); }
        let expect = o.size.div_ceil(o.block_size as u64).max(1);
        if o.blocks as u64 != expect { bail!("blocks = {}, а для размера {} нужно {expect}", o.blocks, o.size); }
        std::fs::create_dir_all(self.part_dir())?;
        let part = self.part_dir().join(format!("{}.part", hex::encode(&o.transfer_id)));
        let f = std::fs::File::create(&part).with_context(|| format!("{}", part.display()))?;
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
        let mut dst = self.dir.join(&name);
        let (stem, ext) = match name.rsplit_once('.') { Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")), _ => (name.clone(), String::new()) };
        let mut k = 1;
        while dst.exists() { dst = self.dir.join(format!("{stem} ({k}){ext}")); k += 1; }
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
