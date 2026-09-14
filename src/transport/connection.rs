//! transport/connection.rs — Асинхронный TCP-транспорт с кадрированием.
//! Умеет читать/писать Frame поверх любого AsyncRead+AsyncWrite стрима
//! (обычный TCP или TLS).

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use super::framing::{Frame, FrameError, FrameReader, ReadItem};

pub const READ_CHUNK: usize = 4096;

/// Обёртка над стримом: читает/пишет Frame.
pub struct FramedStream<S> {
    inner:   S,
    reader:  FrameReader,
    timeout: Duration,
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin + Send> FramedStream<S> {
    pub fn new(stream: S, read_timeout_secs: u64) -> Self {
        Self {
            inner:   stream,
            reader:  FrameReader::new(),
            timeout: Duration::from_secs(read_timeout_secs),
        }
    }

    /// Отправить кадр.
    pub async fn send(&mut self, frame: &Frame) -> Result<(), FrameError> {
        let bytes = frame.encode()?;
        self.inner.write_all(&bytes).await?;
        debug!("→ {}", frame);
        Ok(())
    }

    /// Прочитать следующий кадр (с тайм-аутом).
    pub async fn recv(&mut self) -> Result<Frame, FrameError> {
        loop {
            // Читаем новые данные
            let mut chunk = [0u8; READ_CHUNK];
            let n = timeout(self.timeout, self.inner.read(&mut chunk))
                .await
                .map_err(|_| FrameError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "read timeout",
                )))??;

            if n == 0 {
                return Err(FrameError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed",
                )));
            }

            let items = self.reader.feed(&chunk[..n]);
            for item in items {
                match item {
                    ReadItem::Frame(f) => {
                        debug!("← {}", f);
                        return Ok(f);
                    }
                    ReadItem::Error(e) => {
                        warn!("frame parse error (skipping byte): {}", e);
                    }
                }
            }
        }
    }

    pub fn into_inner(self) -> S { self.inner }
}
