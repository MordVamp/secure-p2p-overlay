//! §13.1: локальная история сообщений и подписанный псевдоним (alias → NodeID).

use std::sync::Arc;
use p2p_overlay::app::history::{History, HistoryEntry};
use p2p_overlay::app::message::AppMessage;
use p2p_overlay::app::messenger::Messenger;
use p2p_overlay::config::NodeConfig;
use p2p_overlay::dht::node::DhtNode;
use p2p_overlay::dht::storage::DhtRecord;
use p2p_overlay::identity::alias::{alias_key, alias_value, owner_of, valid_alias};
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::protocol::payload::encode;
use p2p_overlay::security::e2e::seal;
use p2p_overlay::tunnel::manager::TunnelManager;
use p2p_overlay::types::Contact;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2p-ha-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}
fn ident(tag: &str) -> NodeIdentity { NodeIdentity::load_or_create(&tmp(tag)).unwrap() }

fn entry(i: u64) -> HistoryEntry {
    HistoryEntry { ts_ms: i, dir: "out".into(), peer: "ab".into(), message_id: format!("{i}"),
                   status: "delivered".into(), text: Some(format!("m{i}")), rtt_ms: Some(i), error: None,
                   lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: Some(i) }
}

#[tokio::test]
async fn history_survives_reopen_and_tail_limits() {
    let d = tmp("h1");
    let h = History::open(&d);
    for i in 0..5 { h.append(&entry(i)).await; }
    drop(h);
    let h = History::open(&d);          // «перезапуск узла»
    let t = h.tail(3).await;
    assert_eq!(t.iter().map(|e| e.ts_ms).collect::<Vec<_>>(), vec![2, 3, 4]);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(h.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "история содержит тексты — только владелец");
    }
}

#[tokio::test]
async fn history_skips_corrupted_lines() {
    let d = tmp("h2");
    let h = History::open(&d);
    h.append(&entry(1)).await;
    {
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(h.path()).unwrap().write_all(b"{not json\n").unwrap();
    }
    h.append(&entry(2)).await;
    assert_eq!(h.tail(10).await.len(), 2);
}

#[tokio::test]
async fn received_message_is_logged() {
    let (a, b) = (Arc::new(ident("ha")), ident("hb"));
    let cfg = Arc::new(NodeConfig::default());
    let me = Contact::new(a.node_id, a.public_key_bytes().to_vec(), "127.0.0.1:7600".parse().unwrap());
    let dht = Arc::new(DhtNode::new(me.clone(), cfg.clone()));
    let tm = Arc::new(TunnelManager::new(me, dht.routing.clone(), cfg.clone()));
    let d = tmp("h3");
    let m = Arc::new(Messenger::new(a.clone(), dht.clone(), tm.clone(), tm.forwarder.clone(), dht.metrics.clone(), cfg)
        .with_history(History::open(&d)));
    let mut msg = AppMessage::new_text(b.node_id, a.node_id, "привет");
    msg.sender_pubkey = b.public_key_bytes().to_vec();
    msg.signature = b.sign(&msg.signing_bytes()).to_bytes().to_vec();
    m.handle_incoming(seal(&a.node_id, &a.public_key_bytes(), &encode(&msg).unwrap()).unwrap()).await.unwrap();
    let h = m.history(10).await;
    assert_eq!(h.len(), 1);
    assert_eq!((h[0].dir.as_str(), h[0].status.as_str(), h[0].text.as_deref()), ("in", "received", Some("привет")));
    assert_eq!(h[0].peer, b.node_id.to_hex());
}

#[test]
fn alias_charset_and_length() {
    assert!(valid_alias("alice"));
    assert!(valid_alias("node-07_x"));
    for bad in ["", "Alice", "a b", "../x", "алиса", &"a".repeat(33)] {
        assert!(!valid_alias(bad), "{bad:?} не должен приниматься");
    }
}

#[test]
fn alias_record_owner_is_signing_node() {
    let id = ident("al1");
    let rec = DhtRecord::new_signed(&id, alias_key("alice"), alias_value("alice"), 60);
    assert_eq!(owner_of("alice", &rec).unwrap(), id.node_id);
}

#[test]
fn alias_record_for_other_name_or_key_rejected() {
    let id = ident("al2");
    // запись «bob», выданная за «alice» (подмена ключа)
    let rec = DhtRecord::new_signed(&id, alias_key("bob"), alias_value("bob"), 60);
    assert!(owner_of("alice", &rec).is_err());
    // ключ «alice», но значение «bob»
    let rec = DhtRecord::new_signed(&id, alias_key("alice"), alias_value("bob"), 60);
    assert!(owner_of("alice", &rec).is_err());
}

#[test]
fn alias_record_with_forged_owner_rejected() {
    let (id, eve) = (ident("al3"), ident("al4"));
    let mut rec = DhtRecord::new_signed(&id, alias_key("alice"), alias_value("alice"), 60);
    // Ева пытается присвоить псевдоним, подставив свой NodeID и ключ
    rec.publisher = eve.node_id;
    rec.publisher_key = eve.public_key_bytes().to_vec();
    assert!(owner_of("alice", &rec).is_err());
}
