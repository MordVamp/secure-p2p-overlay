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
        }
    }

    /// Сериализовать для подписи/верификации (без поля signature).
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.message_id);
        buf.push(match self.kind { MessageKind::Text => 1, MessageKind::Ack => 2, MessageKind::System => 3 });
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

    pub fn message_id_hex(&self) -> String { hex::encode(&self.message_id[..4.min(self.message_id.len())]) }
}
