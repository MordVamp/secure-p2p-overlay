//! app/message.rs — Прикладное сообщение E2E.

use serde::{Deserialize, Serialize};
use crate::types::{NodeId, now_ms};

/// Тип прикладного сообщения.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageKind {
    /// Текстовое сообщение (UTF-8).
    Text,
    /// Подтверждение доставки.
    Ack,
    /// Системное уведомление (join/leave/ping).
    System,
    /// Передача файла (§13.2): предложение, блок, подтверждение окна, итог (app/files.rs).
    FileOffer,
    FileBlock,
    FileAck,
    FileDone,
    /// «Печатает…»: пустой payload, без подтверждения и без записи в историю (опция клиента).
    Typing,
    /// Отчёт о прочтении: payload = `ReadBody` (message_id прочитанных сообщений).
    Read,
    /// Сообщение группового чата: payload = `GroupBody`. Группа — на стороне клиентов: отправитель
    /// рассылает копию каждому участнику (своё E2E-шифрование для каждого), состав едет в сообщении.
    GroupText,
    /// Измерительный пакет потока (план звонков, шаг 0): payload = `ProbeBody`. Без подтверждения и истории;
    /// получатель запоминает время прихода — задержка в одну сторону, джиттер, потери.
    Probe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeBody {
    pub stream:  String,
    pub seq:     u32,
    /// Время отправки по часам отправителя (unix ms; на стенде одной машины часы общие).
    pub sent_us: u64,
    pub pad:     Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadBody {
    pub ids: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupBody {
    /// Случайный идентификатор группы (hex), задаёт создатель.
    pub group_id: String,
    pub name:     String,
    /// NodeID участников (hex), включая отправителя.
    pub members:  Vec<String>,
    pub text:     String,
}

impl MessageKind {
    /// Байт типа в подписываемых данных.
    pub fn code(&self) -> u8 {
        match self {
            MessageKind::Text => 1, MessageKind::Ack => 2, MessageKind::System => 3,
            MessageKind::FileOffer => 4, MessageKind::FileBlock => 5, MessageKind::FileAck => 6, MessageKind::FileDone => 7,
            MessageKind::Typing => 8, MessageKind::Read => 9, MessageKind::GroupText => 10, MessageKind::Probe => 11,
        }
    }
}

/// Прикладное E2E-сообщение.
///
/// Передаётся через туннель как payload TunnelData.
/// Подпись покрывает все поля кроме самой signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppMessage {
    /// UUID сообщения (16 байт).
    pub message_id:   Vec<u8>,
    /// Тип сообщения.
    pub kind:         MessageKind,
    /// NodeId отправителя.
    pub from:         NodeId,
    /// NodeId получателя.
    pub to:           NodeId,
    /// Временная метка (unix ms).
    pub timestamp_ms: u64,
    /// Содержимое (текст или бинарные данные).
    pub payload:      Vec<u8>,
    /// Ed25519 подпись over (message_id || kind || from || to || timestamp_ms || payload).
    pub signature:    Vec<u8>,
    /// A6: публичный ключ отправителя. Self-certifying: NodeID = hash(ключ),
    /// поэтому проверка не зависит от таблицы маршрутизации получателя.
    #[serde(default)]
    pub sender_pubkey: Vec<u8>,
}

impl AppMessage {
    /// Создать новое исходящее текстовое сообщение.
    pub fn new_text(from: NodeId, to: NodeId, text: &str) -> Self {
        Self {
            message_id:   uuid::Uuid::new_v4().into_bytes().to_vec(),
            kind:         MessageKind::Text,
            from, to,
            timestamp_ms: now_ms(),
            payload:      text.as_bytes().to_vec(),
            signature:    vec![],  // заполняется через sign()
            sender_pubkey: vec![],
        }
    }

    /// APP_ACK: подтверждение доставки сообщения `acked_id` (payload = его message_id).
    pub fn new_ack(from: NodeId, to: NodeId, acked_id: &[u8]) -> Self {
        Self {
            message_id:   uuid::Uuid::new_v4().into_bytes().to_vec(),
            kind:         MessageKind::Ack,
            from, to,
            timestamp_ms: now_ms(),
            payload:      acked_id.to_vec(),
            signature:    vec![],
            sender_pubkey: vec![],
        }
    }

    /// Сообщение произвольного типа с готовым payload (передача файлов).
    pub fn new_kind(from: NodeId, to: NodeId, kind: MessageKind, payload: Vec<u8>) -> Self {
        Self {
            message_id: uuid::Uuid::new_v4().into_bytes().to_vec(), kind, from, to,
            timestamp_ms: now_ms(), payload, signature: vec![], sender_pubkey: vec![],
        }
    }

    /// Сериализовать для подписи/верификации (без поля signature).
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.message_id);
        buf.push(self.kind.code());
        buf.extend_from_slice(&self.from.0);
        buf.extend_from_slice(&self.to.0);
        buf.extend_from_slice(&self.timestamp_ms.to_be_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn text(&self) -> Option<&str> {
        if self.kind == MessageKind::Text {
            std::str::from_utf8(&self.payload).ok()
        } else { None }
    }

    /// Тело группового сообщения.
    pub fn group(&self) -> Option<GroupBody> {
        if self.kind != MessageKind::GroupText { return None; }
        crate::protocol::payload::decode(&self.payload).ok()
    }

    pub fn message_id_hex(&self) -> String { hex::encode(&self.message_id[..4.min(self.message_id.len())]) }
}
