//! tests/test_framing.rs — Тесты транспортного уровня T1.1–T1.10 (§13 ТЗ).

use bytes::Bytes;
use p2p_overlay::transport::framing::{
    Frame, FrameFlags, FrameReader, MsgType, ReadItem, HEADER_SIZE, HEADER_LEN, PROTOCOL_VERSION,
};
use p2p_overlay::protocol::payload::{
    PingPayload, FindNodeRequest, NodeIdBytes, encode, decode,
};
use p2p_overlay::types::{Contact, NodeId};

fn make_contact() -> Contact {
    Contact::new(NodeId([1u8; 32]), vec![0u8; 32], "127.0.0.1:7001".parse().unwrap())
}

fn make_request_id() -> [u8; 16] {
    uuid::Uuid::new_v4().into_bytes()
}

fn make_ping_frame() -> Frame {
    let payload = encode(&PingPayload {
        sender:       make_contact(),
        timestamp_ms: 1234567890,
    }).unwrap();
    Frame {
        version:    PROTOCOL_VERSION,
        msg_type:   MsgType::Ping,
        flags:      FrameFlags::empty(),
        request_id: *b"\xDE\xAD\xBE\xEF\xCA\xFE\xBA\xBE\x00\x01\x02\x03\x04\x05\x06\x07",
        payload:    Bytes::from(payload),
    }
}

// T1.1: Неполный заголовок (< 24 байт) → FrameReader ждёт, не паникует
#[test]
fn t1_1_short_header() {
    let mut reader = FrameReader::new();
    let partial = [PROTOCOL_VERSION, 0x01, 0, 0]; // только 4 байта из 24
    let items = reader.feed(&partial);
    assert!(items.is_empty(), "Should buffer partial header");
}

// T1.2: Ровно один кадр → разобрать корректно
#[test]
fn t1_2_exact_frame() {
    let frame = make_ping_frame();
    let encoded = frame.encode().unwrap();

    let mut reader = FrameReader::new();
    let items = reader.feed(&encoded);

    assert_eq!(items.len(), 1);
    match &items[0] {
        ReadItem::Frame(f) => {
            assert_eq!(f.msg_type, MsgType::Ping);
            assert_eq!(f.request_id, frame.request_id);
            assert_eq!(f.payload, frame.payload);
        }
        _ => panic!("Expected Frame"),
    }
}

// T1.3: Два склеенных кадра в одном буфере → два Frame
#[test]
fn t1_3_two_frames_one_read() {
    let f1 = make_ping_frame();
    let f2 = Frame {
        version:    PROTOCOL_VERSION,
        msg_type:   MsgType::Pong,
        flags:      FrameFlags::IS_RESPONSE,
        request_id: f1.request_id,
        payload:    Bytes::from_static(b"\x01\x02\x03"),
    };

    let mut buf = f1.encode().unwrap().to_vec();
    buf.extend_from_slice(&f2.encode().unwrap());

    let mut reader = FrameReader::new();
    let items = reader.feed(&buf);

    let frames: Vec<&Frame> = items.iter().filter_map(|i| {
        if let ReadItem::Frame(f) = i { Some(f) } else { None }
    }).collect();
    assert_eq!(frames.len(), 2, "Expected 2 frames, got {}", frames.len());
    let types: Vec<MsgType> = frames.iter().map(|f| f.msg_type).collect();
    assert!(types.contains(&MsgType::Ping));
    assert!(types.contains(&MsgType::Pong));
}

// T1.4: Один кадр в трёх фрагментах → один Frame
#[test]
fn t1_4_fragmented_frame() {
    let frame  = make_ping_frame();
    let encoded = frame.encode().unwrap().to_vec();
    let (p1, rest) = encoded.split_at(8);
    let (p2, p3)   = rest.split_at(16);

    let mut reader = FrameReader::new();
    let r1 = reader.feed(p1);
    let r2 = reader.feed(p2);
    let r3 = reader.feed(p3);

    assert!(r1.is_empty(), "No frame after part 1");
    assert!(r2.is_empty(), "No frame after part 2");
    assert_eq!(r3.len(), 1, "Frame after part 3");
    assert!(matches!(&r3[0], ReadItem::Frame(_)));
}

// T1.5: payload_length > MAX_FRAME_PAYLOAD → ошибка до выделения буфера
#[test]
fn t1_5_payload_too_large() {
    let mut hdr = [0u8; HEADER_SIZE];
    hdr[0] = PROTOCOL_VERSION;
    hdr[1] = 0x01; // PING
    // request_id bytes 4..20 = 0
    let big: u32 = 70_000;
    hdr[20..24].copy_from_slice(&big.to_be_bytes());

    let mut reader = FrameReader::new();
    let items = reader.feed(&hdr);

    assert!(
        items.iter().any(|i| matches!(i, ReadItem::Error(_))),
        "Expected Error for oversized payload"
    );
}

// T1.6: Неверная версия → ReadItem::Error (не паника)
#[test]
fn t1_6_bad_version() {
    let frame = Frame {
        version:    0xFF,
        msg_type:   MsgType::Ping,
        flags:      FrameFlags::empty(),
        request_id: [0u8; 16],
        payload:    Bytes::new(),
    };
    let encoded = frame.encode().unwrap().to_vec();
    let mut reader = FrameReader::new();
    let items = reader.feed(&encoded);
    assert!(
        items.iter().any(|i| matches!(i, ReadItem::Error(_))),
        "Expected Error for bad version"
    );
}

// T1.7: Неизвестный тип → ReadItem::Error
#[test]
fn t1_7_unknown_type() {
    // Вручную собираем кадр с байтом типа 0x99
    let mut hdr = [0u8; HEADER_SIZE];
    hdr[0] = PROTOCOL_VERSION;
    hdr[1] = 0x99; // неизвестный тип
    // payload_length = 0

    let mut reader = FrameReader::new();
    let items = reader.feed(&hdr);

    assert!(
        items.iter().any(|i| matches!(i, ReadItem::Error(_))),
        "Expected Error for unknown type"
    );
}

// T1.8: Ответный Frame содержит тот же request_id, что и запрос
#[test]
fn t1_8_response_same_request_id() {
    let req  = make_ping_frame();
    let resp = Frame::new_response(MsgType::Pong, req.request_id, Bytes::new());
    assert_eq!(resp.request_id, req.request_id);
    assert!(resp.flags.contains(FrameFlags::IS_RESPONSE));
}

// T1.9: PING payload — MessagePack round-trip
#[test]
fn t1_9_ping_roundtrip() {
    let original = PingPayload {
        sender:       make_contact(),
        timestamp_ms: 999_888_777,
    };
    let bytes   = encode(&original).unwrap();
    let decoded: PingPayload = decode(&bytes).unwrap();
    assert_eq!(decoded.timestamp_ms, original.timestamp_ms);
    assert_eq!(decoded.sender.port,  original.sender.port);
}

// T1.10: FIND_NODE_REQUEST payload — MessagePack round-trip
#[test]
fn t1_10_find_node_roundtrip() {
    let original = FindNodeRequest {
        sender:         make_contact(),
        target_node_id: NodeIdBytes([42u8; 32].to_vec()),
    };
    let bytes   = encode(&original).unwrap();
    let decoded: FindNodeRequest = decode(&bytes).unwrap();
    assert_eq!(decoded.target_node_id.0, original.target_node_id.0);
    assert_eq!(decoded.sender.node_id.0, original.sender.node_id.0);
}
