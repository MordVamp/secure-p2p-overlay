//! transport/connection.rs — Async TCP-транспорт с кадрированием.
//! FramedStream<S> работает поверх любого AsyncRead+AsyncWrite (TCP или TLS).

use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use super::framing::{Frame, FrameError, FrameReader, ReadItem};

pub const READ_CHUNK: usize = 4096;

/// Обёртка над стримом: читает/пишет Frame.
pub struct FramedStream<S> {
    inner:       S,
    reader:      FrameReader,
    timeout_ms:  u64,
    pending:     Vec<ReadItem>,
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin + Send> FramedStream<S> {
    pub fn new(stream: S, read_timeout_ms: u64) -> Self {
        Self {
            inner:    stream,
            reader:   FrameReader::new(),
            timeout_ms: read_timeout_ms,
            pending:  Vec::new(),
        }
    }

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
