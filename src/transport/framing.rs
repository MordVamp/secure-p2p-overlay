//! transport/framing.rs — Кадрирование поверх TCP-потока байтов.
//!
//! Формат заголовка (24 байта, big-endian):
//!   version        1 B  — версия протокола (текущая = 1)
//!   msg_type       1 B  — тип сообщения (MsgType)
//!   flags          2 B  — признаки IS_RESPONSE / IS_ERROR / MORE_FRAGS
//!   request_id    16 B  — UUID v4 (сопоставление запрос/ответ + дедупликация)
//!   payload_length 4 B  — длина payload (0..MAX_FRAME_PAYLOAD)
//! Итого заголовок: 24 байта.
//! payload: 0..MAX_FRAME_PAYLOAD байт.

use std::fmt;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u8  = 1;
pub const HEADER_SIZE: usize    = 24;
pub const HEADER_LEN: usize = HEADER_SIZE;
pub const MAX_FRAME_PAYLOAD: usize = 65_536;

// ── Типы сообщений ────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MsgType {
    // DHT
    Ping              = 0x01,
    Pong              = 0x02,
    FindNodeRequest   = 0x03,
    FindNodeResponse  = 0x04,
    StoreRequest      = 0x05,
    StoreResponse     = 0x06,
    FindValueRequest  = 0x07,
    FindValueResponse = 0x08,
    // Туннель
    TunnelBuild       = 0x10,
    TunnelBuildOk     = 0x11,
    TunnelBuildFail   = 0x12,
    TunnelData        = 0x13,
    TunnelAck         = 0x14,
    TunnelClose       = 0x15,
    // Приложение
    AppMessage        = 0x20,
    AppAck            = 0x21,
    // Служебное
    Error = 0x7F,
}

impl TryFrom<u8> for MsgType {
    type Error = FrameError;
    fn try_from(v: u8) -> Result<Self, FrameError> {
        match v {
            0x01 => Ok(Self::Ping),
            0x02 => Ok(Self::Pong),
            0x03 => Ok(Self::FindNodeRequest),
            0x04 => Ok(Self::FindNodeResponse),
            0x05 => Ok(Self::StoreRequest),
            0x06 => Ok(Self::StoreResponse),
            0x07 => Ok(Self::FindValueRequest),
            0x08 => Ok(Self::FindValueResponse),
            0x10 => Ok(Self::TunnelBuild),
            0x11 => Ok(Self::TunnelBuildOk),
            0x12 => Ok(Self::TunnelBuildFail),
            0x13 => Ok(Self::TunnelData),
            0x14 => Ok(Self::TunnelAck),
            0x15 => Ok(Self::TunnelClose),
            0x20 => Ok(Self::AppMessage),
            0x21 => Ok(Self::AppAck),
            0x7F => Ok(Self::Error),
            _ => Err(FrameError::UnknownMsgType(v)),
        }
    }
}

// ── Флаги ─────────────────────────────────────────────────────────────────────

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameFlags: u16 {
        const IS_RESPONSE = 0x0001;
        const IS_ERROR    = 0x0002;
        const MORE_FRAGS  = 0x0004;
    }
}

// ── Кадр ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Frame {
    pub version:    u8,
    pub msg_type:   MsgType,
    pub flags:      FrameFlags,
    /// 16 байт UUID v4.
    pub request_id: [u8; 16],
    pub payload:    Bytes,
}

impl Frame {
    pub fn new(msg_type: MsgType, payload: Bytes) -> Self {
        Self {
            version:    PROTOCOL_VERSION,
            msg_type,
            flags:      FrameFlags::empty(),
            request_id: Uuid::new_v4().into_bytes(),
            payload,
        }
    }

    pub fn response(msg_type: MsgType, request_id: [u8; 16], payload: Bytes) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            msg_type,
            flags: FrameFlags::IS_RESPONSE,
            request_id,
            payload,
        }
    }

    /// Сериализует кадр в BytesMut (заголовок + payload).
    pub fn encode(&self) -> Result<BytesMut, FrameError> {
        if self.payload.len() > MAX_FRAME_PAYLOAD {
            return Err(FrameError::PayloadTooLarge(self.payload.len()));
        }
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + self.payload.len());
        buf.put_u8(self.version);
        buf.put_u8(self.msg_type as u8);
        buf.put_u16(self.flags.bits());
        buf.put_slice(&self.request_id);
        buf.put_u32(self.payload.len() as u32);
        buf.put_slice(&self.payload);
        Ok(buf)
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Frame({:?}, {} B)", self.msg_type, self.payload.len())
    }
}

// ── Конечный автомат чтения кадров ───────────────────────────────────────────

/// Накапливает входящие байты и извлекает полные кадры.
/// Корректно обрабатывает:
///   - частичный заголовок (данные пришли по частям)
///   - частичный payload
///   - несколько кадров в одном буфере (TCP coalescing)
///   - версию, тип, размер — проверяются до выделения буфера
#[derive(Default)]
pub struct FrameReader {
    buf: BytesMut,
}

#[derive(Debug)]
/// Результат одного шага чтения: либо готовый кадр, либо ошибка разбора.
pub enum ReadItem {
    Frame(Frame),
    /// Ошибка разбора; reader сбросил один байт и пытается ресинхронизоваться.
    Error(FrameError),
    /// Соединение закрыто другой стороной (EOF).
    Eof,
}

impl FrameReader {
    pub fn new() -> Self { Self::default() }

    /// Подать новые байты; вернуть все полностью прочитанные кадры/ошибки.
    pub fn feed(&mut self, data: &[u8]) -> Vec<ReadItem> {
        self.buf.extend_from_slice(data);
        let mut results = Vec::new();

        loop {
            if self.buf.len() < HEADER_SIZE {
                break;
            }

            // ── Разбор заголовка (НЕ потребляем buf до успешной проверки) ────
            let version        = self.buf[0];
            let type_byte      = self.buf[1];
            let flags_raw      = u16::from_be_bytes([self.buf[2], self.buf[3]]);
            let mut rid = [0u8; 16];
            rid.copy_from_slice(&self.buf[4..20]);
            let payload_length = u32::from_be_bytes([
                self.buf[20], self.buf[21], self.buf[22], self.buf[23],
            ]) as usize;

            // Проверка до выделения буфера
            if version != PROTOCOL_VERSION {
                results.push(ReadItem::Error(FrameError::UnknownVersion(version)));
                self.buf.advance(1);
                continue;
            }
            if payload_length > MAX_FRAME_PAYLOAD {
                results.push(ReadItem::Error(FrameError::PayloadTooLarge(payload_length)));
                self.buf.advance(1);
                continue;
            }
            let msg_type = match MsgType::try_from(type_byte) {
                Ok(t) => t,
                Err(e) => {
                    results.push(ReadItem::Error(e));
                    self.buf.advance(1);
                    continue;
                }
            };

            let total = HEADER_SIZE + payload_length;
            if self.buf.len() < total {
                break; // Payload ещё не весь пришёл
            }

            // Потребляем заголовок
            self.buf.advance(HEADER_SIZE);
            let payload = self.buf.split_to(payload_length).freeze();

            let flags = FrameFlags::from_bits_truncate(flags_raw);

            results.push(ReadItem::Frame(Frame {
                version,
                msg_type,
                flags,
                request_id: rid,
                payload,
            }));
        }

        results
    }

    pub fn clear(&mut self) { self.buf.clear(); }
}

// ── Ошибки ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("unknown protocol version: {0}")]
    UnknownVersion(u8),
    #[error("unknown message type: 0x{0:02X}")]
    UnknownMsgType(u8),
    #[error("payload too large: {0} > MAX_FRAME_PAYLOAD")]
    PayloadTooLarge(usize),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}


// ── Вспомогательные конструкторы ─────────────────────────────────────────────

impl Frame {
    /// Ответный Frame с тем же request_id.
    pub fn new_response(msg_type: MsgType, request_id: [u8; 16], payload: Bytes) -> Self {
        Self { version: PROTOCOL_VERSION, msg_type, flags: FrameFlags::IS_RESPONSE, request_id, payload }
    }

    /// ERROR-кадр для неверной версии протокола.
    pub fn error_version(request_id: [u8; 16]) -> Self {
        use crate::protocol::payload::{encode as mp_encode, ErrorPayload, error_codes};
        let err = ErrorPayload { code: error_codes::BAD_VERSION, description: "bad version".into() };
        let bytes = mp_encode(&err).unwrap_or_default();
        Self {
            version: PROTOCOL_VERSION, msg_type: MsgType::Error,
            flags: FrameFlags::IS_RESPONSE | FrameFlags::IS_ERROR,
            request_id, payload: Bytes::from(bytes),
        }
    }

    /// Добавить флаг IS_ERROR.
    pub fn with_error_flag(mut self) -> Self {
        self.flags |= FrameFlags::IS_ERROR;
        self
    }
}

impl ReadItem {
    pub fn is_eof(&self) -> bool { matches!(self, ReadItem::Eof) }
}
