//! transport/connection.rs — Async TCP-транспорт с кадрированием.
//! FramedStream<S> работает поверх любого AsyncRead+AsyncWrite (TCP или TLS).

use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use super::framing::{Frame, FrameError, FrameReader, ReadItem};

pub const READ_CHUNK: usize = 4096;

/// Любой дуплексный поток (TCP или TLS) — для динамической диспетчеризации в `dial`.
pub trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}
pub type BoxedIo = Box<dyn Io>;

/// Обёртка над стримом: читает/пишет Frame.
pub struct FramedStream<S> {
    inner:       S,
    reader:      FrameReader,
    timeout_ms:  u64,
    pending:     Vec<ReadItem>,
    /// NodeID пира, подтверждённый TLS-рукопожатием (None — соединение без TLS).
    peer_node_id: Option<crate::types::NodeId>,
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin + Send> FramedStream<S> {
    pub fn new(stream: S, read_timeout_ms: u64) -> Self {
        Self {
            inner:    stream,
            reader:   FrameReader::new(),
            timeout_ms: read_timeout_ms,
            pending:  Vec::new(),
            peer_node_id: None,
        }
    }

    /// Запомнить NodeID пира, полученный из TLS-сертификата.
    pub fn with_peer(mut self, id: crate::types::NodeId) -> Self {
        self.peer_node_id = Some(id);
        self
    }

    /// NodeID пира по TLS (если соединение защищено).
    pub fn peer_node_id(&self) -> Option<&crate::types::NodeId> { self.peer_node_id.as_ref() }

    /// Отправить кадр целиком.
    pub async fn send(&mut self, frame: &Frame) -> Result<(), FrameError> {
        let bytes = frame.encode()?;
        self.inner.write_all(&bytes).await?;
        self.inner.flush().await?;   // AKE буферизует запись — отправить сразу
        debug!("→ {:?} len={}", frame.msg_type, bytes.len());
        Ok(())
    }

    /// Вернуть следующий `ReadItem` (Frame, Error или Eof).
    pub async fn recv_item(&mut self) -> ReadItem {
        loop {
            // Сначала возвращаем ранее разобранные элементы из буфера
            if !self.pending.is_empty() {
                return self.pending.remove(0);
            }

            // Читаем новую порцию данных с тайм-аутом
            let mut chunk = [0u8; READ_CHUNK];
            let n = match timeout(
                Duration::from_millis(self.timeout_ms),
                self.inner.read(&mut chunk),
            ).await {
                Err(_) => return ReadItem::Error(FrameError::Io(io::Error::new(
                    io::ErrorKind::TimedOut, "read timeout",
                ))),
                // TLS: пир закрыл TCP без close_notify — для нас это обычный конец потока
                Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return ReadItem::Eof,
                Ok(Err(e)) => return ReadItem::Error(FrameError::Io(e)),
                Ok(Ok(0)) => return ReadItem::Eof,
                Ok(Ok(n)) => n,
            };

            let items = self.reader.feed(&chunk[..n]);
            self.pending.extend(items);
        }
    }

    /// Для обратной совместимости: ждём первый Frame, остальные ошибки логируем.
    pub async fn recv(&mut self) -> Result<Frame, FrameError> {
        loop {
            match self.recv_item().await {
                ReadItem::Frame(f) => {
                    debug!("← {:?} len={}", f.msg_type, f.payload.len());
                    return Ok(f);
                }
                ReadItem::Error(e) => {
                    warn!("frame parse error: {e}");
                }
                ReadItem::Eof => return Err(FrameError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof, "connection closed",
                ))),
            }
        }
    }

    pub fn into_inner(self) -> S { self.inner }

    /// Разобрать на части: поток, уже прочитанные байты/кадры и NodeID пира — чтобы читать и писать
    /// из разных задач (соединение с несколькими запросами одновременно).
    pub fn into_parts(self) -> (S, FrameRead<()>, Option<crate::types::NodeId>) {
        let rest = FrameRead { inner: (), reader: self.reader, pending: self.pending.into(), read_ms: self.timeout_ms };
        (self.inner, rest, self.peer_node_id)
    }
}

/// Чтение кадров из половины потока. Отличает **простой** между кадрами (ждём до `idle_ms`, затем `None`)
/// от **недочитанного кадра** (ждём не дольше `READ_TIMEOUT_MS`, затем ошибка) — для долгих соединений.
pub struct FrameRead<R> {
    inner:   R,
    reader:  FrameReader,
    pending: std::collections::VecDeque<ReadItem>,
    read_ms: u64,
}

impl FrameRead<()> {
    /// Подставить половину потока для чтения (буфер, начатый `FramedStream`, сохраняется).
    pub fn with<R>(self, inner: R) -> FrameRead<R> {
        FrameRead { inner, reader: self.reader, pending: self.pending, read_ms: self.read_ms }
    }
}

impl<R: tokio::io::AsyncRead + Unpin> FrameRead<R> {
    /// Следующий элемент; `None` — соединение простаивало `idle_ms` без начатого кадра.
    pub async fn next(&mut self, idle_ms: u64) -> Option<ReadItem> {
        loop {
            if let Some(it) = self.pending.pop_front() { return Some(it); }
            let wait = if self.reader.buffered() == 0 { idle_ms } else { self.read_ms };
            let mut chunk = [0u8; READ_CHUNK];
            let n = match timeout(Duration::from_millis(wait), self.inner.read(&mut chunk)).await {
                Err(_) if self.reader.buffered() == 0 => return None,
                Err(_) => return Some(ReadItem::Error(FrameError::Io(io::Error::new(
                    io::ErrorKind::TimedOut, "read timeout: кадр не дочитан")))),
                Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Some(ReadItem::Eof),
                Ok(Err(e)) => return Some(ReadItem::Error(FrameError::Io(e))),
                Ok(Ok(0)) => return Some(ReadItem::Eof),
                Ok(Ok(n)) => n,
            };
            self.pending.extend(self.reader.feed(&chunk[..n]));
        }
    }
}

/// Записать кадр целиком в половину потока.
pub async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, frame: &Frame) -> Result<(), FrameError> {
    let bytes = frame.encode()?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

// Специализация для TcpStream: выдать поток для spawn_conn_reader
impl<S> FramedStream<S> {
    /// Отдать внутренний TcpStream с переносом накопленного буфера.
    /// Используется в RpcClient после send() для запуска reader-задачи.
    pub fn into_raw_stream(self) -> FramedStream<S> {
        // Мы уже FramedStream — просто возвращаем себя.
        // Накопленный буфер FrameReader переходит вместе.
        self
    }
}
