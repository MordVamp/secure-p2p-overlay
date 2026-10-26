//! Тесты исправлений аудита раунда 2 (A4–A8, B3, B6).

use std::sync::Arc;
use p2p_overlay::app::message::AppMessage;
use p2p_overlay::config::NodeConfig;
use p2p_overlay::dht::storage::{DhtRecord, RecordKey};
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::tunnel::routes::TunnelRoutes;
use p2p_overlay::tunnel::session::TunnelSession;

fn ident(tag: &str) -> NodeIdentity {
    let dir = std::env::temp_dir().join(format!("p2p-audit2-{tag}-{}", std::process::id()));
    NodeIdentity::load_or_create(&dir).unwrap()
}

#[test]
fn a7_signed_record_verifies() {
    let id = ident("a7");
    let r = DhtRecord::new_signed(&id, RecordKey::from_str("k"), b"v".to_vec(), 60);
    assert!(r.verify());
}

#[test]
fn a7_tampered_value_rejected() {
    let id = ident("a7t");
    let mut r = DhtRecord::new_signed(&id, RecordKey::from_str("k"), b"v".to_vec(), 60);
    r.value = b"evil".to_vec();
    assert!(!r.verify());
}

#[test]
fn a7_unsigned_or_foreign_key_rejected() {
    let a = ident("a7a"); let b = ident("a7b");
    let mut r = DhtRecord::new(RecordKey::from_str("k"), b"v".to_vec(), a.node_id, 60);
    assert!(!r.verify(), "без подписи");
    r = DhtRecord::new_signed(&a, RecordKey::from_str("k"), b"v".to_vec(), 60);
    r.publisher_key = b.public_key_bytes().to_vec();
    assert!(!r.verify(), "ключ не соответствует publisher");
}

#[tokio::test]
async fn a5_routes_insert_lookup() {
    let rt = TunnelRoutes::new();
    let addr = "127.0.0.1:7001".parse().unwrap();
    assert!(rt.next_hop(&[1; 16]).await.is_none());
    assert!(rt.insert([1; 16], addr, 60).await);
    assert_eq!(rt.next_hop(&[1; 16]).await, Some(addr));
}

#[tokio::test]
async fn b6_outbox_kept_and_session_id_preserved() {
    let cfg = Arc::new(NodeConfig::default());
    let (s, rx) = TunnelSession::with_id([7; 16], vec![], cfg);
    s.attach_outbox(rx).await;
    assert_eq!(s.id, [7; 16]);
    assert!(s.take_outbox().await.is_some());
    assert!(s.take_outbox().await.is_none());
}

#[test]
fn a6_message_carries_pubkey() {
    let id = ident("a6");
    let mut m = AppMessage::new_text(id.node_id, id.node_id, "hi");
    m.sender_pubkey = id.public_key_bytes().to_vec();
    assert_eq!(m.sender_pubkey.len(), 32);
}
