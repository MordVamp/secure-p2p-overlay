//! tests/test_app.rs — Тесты прикладного уровня T7.1–T7.5.

use p2p_overlay::app::message::{AppMessage, MessageKind};
use p2p_overlay::types::NodeId;

fn make_id(b: u8) -> NodeId { NodeId([b; 32]) }

// T7.1: Создание текстового сообщения — поля заполнены корректно
#[test]
fn t7_1_text_message_creation() {
    let from = make_id(0x01);
    let to   = make_id(0x02);
    let msg  = AppMessage::new_text(from, to, "hello world");
    assert_eq!(msg.kind, MessageKind::Text);
    assert_eq!(msg.from, from);
    assert_eq!(msg.to, to);
    assert_eq!(msg.text(), Some("hello world"));
    assert_eq!(msg.message_id.len(), 16, "UUID must be 16 bytes");
    assert!(msg.timestamp_ms > 0);
}

// T7.2: signing_bytes() детерминированы для одного сообщения
#[test]
fn t7_2_signing_bytes_deterministic() {
    let msg = AppMessage::new_text(make_id(0x01), make_id(0x02), "test");
    let b1 = msg.signing_bytes();
    let b2 = msg.signing_bytes();
    assert_eq!(b1, b2, "signing_bytes must be deterministic");
    assert!(!b1.is_empty());
}

// T7.3: Разные сообщения → разные signing_bytes
#[test]
fn t7_3_different_messages_different_signing_bytes() {
    let m1 = AppMessage::new_text(make_id(0x01), make_id(0x02), "hello");
    let m2 = AppMessage::new_text(make_id(0x01), make_id(0x02), "world");
    assert_ne!(m1.signing_bytes(), m2.signing_bytes());
    // Также разные message_id (UUID)
    assert_ne!(m1.message_id, m2.message_id);
}

// T7.4: Сериализация AppMessage → десериализация round-trip (MessagePack)
#[test]
fn t7_4_message_serialize_roundtrip() {
    use p2p_overlay::protocol::payload::{encode, decode};
    let original = AppMessage::new_text(make_id(0xAA), make_id(0xBB), "round-trip test");
    let bytes = encode(&original).unwrap();
    let decoded: AppMessage = decode(&bytes).unwrap();
    assert_eq!(decoded.kind, original.kind);
    assert_eq!(decoded.from, original.from);
    assert_eq!(decoded.to, original.to);
    assert_eq!(decoded.text(), original.text());
    assert_eq!(decoded.message_id, original.message_id);
}

// T7.5: text() возвращает None для Ack/System
#[test]
fn t7_5_text_only_for_text_kind() {
    let mut msg = AppMessage::new_text(make_id(1), make_id(2), "data");
    msg.kind = MessageKind::Ack;
    assert_eq!(msg.text(), None, "Ack message must not return text");

    msg.kind = MessageKind::System;
    assert_eq!(msg.text(), None, "System message must not return text");
}

// T7.6: MAX_SEGMENT_SIZE константа = 48 KiB
#[test]
fn t7_6_max_segment_size() {
    use p2p_overlay::tunnel::forwarder::MAX_SEGMENT_SIZE;
    assert_eq!(MAX_SEGMENT_SIZE, 49_152, "MAX_SEGMENT_SIZE must be 48 KiB");
}
