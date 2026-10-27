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
