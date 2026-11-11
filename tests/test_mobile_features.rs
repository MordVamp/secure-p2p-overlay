//! Возможности для мобильного клиента: «только клиент», события (push), «печатает», отчёты
//! о прочтении, групповые сообщения, офлайн-очередь (почтовый ящик DHT), удаление переписки,
//! номер безопасности, шифрование identity.key.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::Mutex;

use p2p_overlay::app::history::{History, HistoryEntry};
use p2p_overlay::app::mailbox::{self, Outbox, Queued};
use p2p_overlay::app::message::{AppMessage, GroupBody, MessageKind, ReadBody};
use p2p_overlay::app::messenger::Messenger;
use p2p_overlay::config::NodeConfig;
use p2p_overlay::dht::node::DhtNode;
use p2p_overlay::dht::routing_table::RoutingTable;
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::protocol::payload::{self, encode, TunnelBuildPayload};
use p2p_overlay::rpc::{PendingRpc, RpcHandler};
use p2p_overlay::security::e2e::seal;
use p2p_overlay::transport::framing::{Frame, FrameFlags, MsgType};
use p2p_overlay::tunnel::manager::TunnelManager;
use p2p_overlay::tunnel::routes::TunnelRoutes;
use p2p_overlay::types::{now_ms, Contact, NodeId};

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2p-mob-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn ident(tag: &str) -> NodeIdentity { NodeIdentity::load_or_create(&dir(tag)).unwrap() }

fn contact(id: &NodeIdentity, port: u16) -> Contact {
    Contact::new(id.node_id, id.public_key_bytes().to_vec(), format!("127.0.0.1:{port}").parse().unwrap())
}

/// Messenger без сети, с историей и очередью в своём каталоге.
fn messenger(id: &Arc<NodeIdentity>, tag: &str, cfg: NodeConfig) -> (Arc<Messenger>, std::path::PathBuf) {
    let cfg = Arc::new(cfg);
    let me = contact(id, 7500);
    let dht = Arc::new(DhtNode::new(me.clone(), cfg.clone()));
    let tm = Arc::new(TunnelManager::new(me, dht.routing.clone(), cfg.clone()));
    let d = dir(&format!("{tag}-state"));
    std::fs::create_dir_all(&d).unwrap();
    let m = Messenger::new(id.clone(), dht.clone(), tm.clone(), tm.forwarder.clone(), dht.metrics.clone(), cfg)
        .with_history(History::open(&d)).with_outbox(&d);
    (Arc::new(m), d)
}

fn wire(from: &NodeIdentity, to: &NodeIdentity, mut m: AppMessage) -> Vec<u8> {
    m.sender_pubkey = from.public_key_bytes().to_vec();
    m.signature = from.sign(&m.signing_bytes()).to_bytes().to_vec();
    seal(&to.node_id, &to.public_key_bytes(), &encode(&m).unwrap()).unwrap()
}

fn kind(from: &NodeIdentity, to: &NodeIdentity, k: MessageKind, body: Vec<u8>) -> AppMessage {
    AppMessage::new_kind(from.node_id, to.node_id, k, body)
}

// ── «только клиент» ─────────────────────────────────────────────────────────

fn handler(cfg: NodeConfig) -> RpcHandler {
    let id = ident(&format!("relay-{}", cfg.tunnel.relay));
    let me = contact(&id, 7400);
    RpcHandler::new(me.clone(), Arc::new(Mutex::new(RoutingTable::new(me.node_id, 3))),
                    Arc::new(PendingRpc::new()), Arc::new(cfg))
        .with_services(Arc::new(Default::default()), Arc::new(TunnelRoutes::new()), None)
}

fn build(init: &Contact, next: Option<Contact>, tid: [u8; 16]) -> Frame {
    Frame { version: p2p_overlay::protocol::PROTOCOL_VERSION, msg_type: MsgType::TunnelBuild, flags: FrameFlags::empty(),
            request_id: [7; 16], payload: Bytes::from(payload::encode(&TunnelBuildPayload {
                tunnel_id: tid.to_vec(), initiator: init.clone(), hop_index: 0, total_hops: if next.is_some() { 2 } else { 1 },
                ttl_seconds: 60, timestamp_ms: 0, next_hop: next,
            }).unwrap()) }
}

#[tokio::test]
async fn client_only_node_refuses_to_relay_but_accepts_own_tunnels() {
    let mut cfg = NodeConfig::default();
    cfg.tunnel.relay = false;
    let h = handler(cfg);
    let (init, nxt) = (contact(&ident("ci"), 7401), contact(&ident("cn"), 7402));
    let e = h.handle_tunnel_build(&build(&init, Some(nxt), [1; 16])).await.unwrap_err();
    assert!(e.to_string().contains("только клиент"), "{e}");
    // конечный узел туннеля (next_hop нет) — можно
    assert!(h.handle_tunnel_build(&build(&init, None, [2; 16])).await.is_ok());
}

#[test]
fn contact_relay_flag_defaults_to_true_for_old_records() {
    // запись без поля relay (старый формат) — узел считается ретранслятором
    let id = ident("old");
    let mut c = contact(&id, 7000);
    let v = serde_json::to_value(&c).unwrap();
    let mut m = v.as_object().unwrap().clone();
    m.remove("relay");
    let back: Contact = serde_json::from_value(serde_json::Value::Object(m)).unwrap();
    assert!(back.relay);
    c.relay = false;
    let back: Contact = payload::decode(&payload::encode(&c).unwrap()).unwrap();
    assert!(!back.relay, "флаг переносится в FIND_NODE-ответах");
}

// ── события, «печатает», прочтение ────────────────────────────────────────

#[tokio::test]
async fn typing_is_published_as_event_and_expires() {
    let (a, b) = (Arc::new(ident("ta")), ident("tb"));
    let (ma, _) = messenger(&a, "ta", NodeConfig::default());
    let since = ma.events.last_seq();
    ma.handle_incoming(wire(&b, &a, kind(&b, &a, MessageKind::Typing, encode(&()).unwrap()))).await.unwrap();
    let (_, ev) = ma.events.wait(since, Duration::from_millis(100)).await;
    assert!(ev.iter().any(|e| e["kind"] == "typing" && e["peer"] == b.node_id.to_hex()));
    assert!(ma.typing_now().await.contains_key(&b.node_id.to_hex()));
    // «печатает» в историю не попадает
    assert!(ma.history(10).await.is_empty());
}

#[tokio::test]
async fn events_wait_times_out_empty_and_wakes_on_emit() {
    let a = Arc::new(ident("ea"));
    let (ma, _) = messenger(&a, "ea", NodeConfig::default());
    let t0 = std::time::Instant::now();
    let (_, ev) = ma.events.wait(ma.events.last_seq(), Duration::from_millis(150)).await;
    assert!(ev.is_empty() && t0.elapsed() >= Duration::from_millis(140));
    let bus = ma.events.clone();
    let since = bus.last_seq();
    let waiter = tokio::spawn(async move { bus.wait(since, Duration::from_secs(5)).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    ma.events.emit("test", serde_json::json!({})).await;
    let (_, ev) = tokio::time::timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap();
    assert_eq!(ev.len(), 1, "долгий запрос просыпается сразу при событии");
}

#[tokio::test]
async fn read_receipt_marks_only_own_messages_to_that_peer() {
    let (a, b, eve) = (Arc::new(ident("ra")), ident("rb"), ident("re"));
    let (ma, d) = messenger(&a, "ra", NodeConfig::default());
    let h = History::open(&d);
    let id = [9u8; 16];
    let out = HistoryEntry { ts_ms: now_ms(), dir: "out".into(), peer: b.node_id.to_hex(), message_id: hex::encode(id),
        status: "delivered".into(), text: Some("hi".into()), rtt_ms: None, error: None, lookup_ms: None,
        tunnel_build_ms: None, tunnel_send_ms: None, group: None, group_name: None, members: None, read_ms: None };
    h.append(&out).await;
    // Ева «читает» чужое сообщение — не применяется
    let body = encode(&ReadBody { ids: vec![id.to_vec()] }).unwrap();
    ma.handle_incoming(wire(&eve, &a, kind(&eve, &a, MessageKind::Read, body.clone()))).await.unwrap();
    assert!(ma.history(10).await[0].read_ms.is_none());
    ma.handle_incoming(wire(&b, &a, kind(&b, &a, MessageKind::Read, body))).await.unwrap();
    let hist = ma.history(10).await;
    assert_eq!(hist.len(), 1, "строка-обновление не видна как отдельная запись");
    assert!(hist[0].read_ms.is_some(), "получатель прочитал");
}

// ── группы ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn group_message_from_member_is_logged_with_group() {
    let (a, b) = (Arc::new(ident("ga")), ident("gb"));
    let (ma, _) = messenger(&a, "ga", NodeConfig::default());
    let g = GroupBody { group_id: "ab12".into(), name: "Курсовая".into(),
                        members: vec![a.node_id.to_hex(), b.node_id.to_hex()], text: "всем привет".into() };
    ma.handle_incoming(wire(&b, &a, kind(&b, &a, MessageKind::GroupText, encode(&g).unwrap()))).await.unwrap();
    let h = ma.history(10).await;
    assert_eq!(h[0].group.as_deref(), Some("ab12"));
    assert_eq!(h[0].text.as_deref(), Some("всем привет"));
    assert_eq!(h[0].peer, b.node_id.to_hex(), "автор группового сообщения сохраняется");
}

#[tokio::test]
async fn group_message_from_non_member_rejected() {
    let (a, b, eve) = (Arc::new(ident("gna")), ident("gnb"), ident("gne"));
    let (ma, _) = messenger(&a, "gna", NodeConfig::default());
    let g = GroupBody { group_id: "cd34".into(), name: "x".into(),
                        members: vec![a.node_id.to_hex(), b.node_id.to_hex()], text: "spam".into() };
    assert!(ma.handle_incoming(wire(&eve, &a, kind(&eve, &a, MessageKind::GroupText, encode(&g).unwrap()))).await.is_err());
    assert!(ma.history(10).await.is_empty());
}

#[tokio::test]
async fn delete_history_removes_only_that_conversation() {
    let (a, b, c) = (Arc::new(ident("da")), ident("db"), ident("dc"));
    let (ma, _) = messenger(&a, "da", NodeConfig::default());
    for (who, txt) in [(&b, "от b"), (&c, "от c")] {
        ma.handle_incoming(wire(who, &a, AppMessage::new_text(who.node_id, a.node_id, txt))).await.unwrap();
    }
    assert_eq!(ma.delete_history(&b.node_id.to_hex()).await.unwrap(), 1);
    let h = ma.history(10).await;
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].peer, c.node_id.to_hex());
}

// ── офлайн-доставка ───────────────────────────────────────────────────────

#[test]
fn outbox_limits_and_expiry() {
    let d = dir("ob");
    std::fs::create_dir_all(&d).unwrap();
    let to = ident("obt").node_id;
    let mut o = Outbox::load(&d);
    for i in 0..5 {
        o.push(&to, Queued { id: format!("{i:02}"), ts_ms: now_ms() - (4 - i) * 1000, envelope: "aa".repeat(10) }, 3);
    }
    assert_eq!(o.body(&to).msgs.len(), 3, "лимит max_messages — старые вытесняются");
    assert!(!o.contains(&to, "00") && o.contains(&to, "04"));
    // переживает перезапуск
    let mut o2 = Outbox::load(&d);
    assert!(o2.remove(&to, "04"));
    assert!(!o2.remove(&to, "04"));
    assert_eq!(o2.expire(1500).len(), 1, "сообщения старше срока выбрасываются");
    assert_eq!(o2.body(&to).msgs.len(), 1);
    assert_ne!(mailbox::key(&to, &NodeId([1; 32])).0, mailbox::key(&NodeId([1; 32]), &to).0, "ящик направленный");
}

#[tokio::test]
async fn undeliverable_message_is_queued_and_removed_by_ack() {
    let (a, b) = (Arc::new(ident("qa")), ident("qb"));
    let (ma, _) = messenger(&a, "qa", NodeConfig::default());
    // ключ b известен из его прежнего сообщения; туннель к b построить негде (сети нет)
    ma.handle_incoming(wire(&b, &a, AppMessage::new_text(b.node_id, a.node_id, "привет"))).await.unwrap();
    let rep = ma.send_text_acked(b.node_id, "ответ, пока b офлайн", Duration::from_millis(300)).await.unwrap();
    assert!(rep.queued, "недоставленное уходит в почтовый ящик");
    assert_eq!(ma.outbox_summary().await.get(&b.node_id.to_hex()), Some(&1));
    let id = hex::decode(&rep.message_id).unwrap();
    assert_eq!(ma.history(10).await.last().unwrap().status, "queued");
    // b забрал письмо из ящика и прислал APP_ACK
    ma.handle_incoming(wire(&b, &a, AppMessage::new_ack(b.node_id, a.node_id, &id))).await.unwrap();
    assert!(ma.outbox_summary().await.get(&b.node_id.to_hex()).is_none());
    assert_eq!(ma.history(10).await.last().unwrap().status, "delivered", "статус обновлён");
}

#[tokio::test]
async fn mailbox_disabled_means_plain_failure() {
    let (a, b) = (Arc::new(ident("na")), ident("nb"));
    let mut cfg = NodeConfig::default();
    cfg.mailbox.enabled = false;
    let (ma, _) = messenger(&a, "na", cfg);
    ma.handle_incoming(wire(&b, &a, AppMessage::new_text(b.node_id, a.node_id, "x"))).await.unwrap();
    assert!(ma.send_text_acked(b.node_id, "y", Duration::from_millis(200)).await.is_err());
}

// ── номер безопасности, шифрование ключа ─────────────────────────────────

#[test]
fn safety_number_is_symmetric_and_pair_specific() {
    use p2p_overlay::gui::node_gui::safety_number;
    let (a, b, c) = (NodeId([1; 32]), NodeId([2; 32]), NodeId([3; 32]));
    let s = safety_number(&a, &b);
    assert_eq!(s, safety_number(&b, &a), "у обоих собеседников одинаковый");
    assert_ne!(s, safety_number(&a, &c));
    assert_eq!(s.split(' ').count(), 12);
    assert!(s.split(' ').all(|g| g.len() == 5 && g.bytes().all(|x| x.is_ascii_digit())));
}

#[test]
fn identity_key_encrypted_with_kek() {
    // явный KEK, без окружения: остальные тесты файла параллельно создают ключи через load_or_create,
    // который читает и удаляет P2P_IDENTITY_KEK (проверка окружения — tests/test_identity_kek_env.rs)
    let d = dir("kek");
    let plain = NodeIdentity::load_or_create_with_kek(&d, None).unwrap();
    assert!(d.join("identity.key").exists());
    let kek = [0x11u8; 32];
    let enc = NodeIdentity::load_or_create_with_kek(&d, Some(&kek)).unwrap();
    assert_eq!(enc.node_id, plain.node_id, "миграция сохраняет NodeID");
    assert!(!d.join("identity.key").exists() && d.join("identity.key.enc").exists(), "открытая копия удалена");
    // без KEK — ошибка, а не новый ключ
    assert!(NodeIdentity::load_or_create_with_kek(&d, None).is_err());
    assert!(NodeIdentity::load_or_create_with_kek(&d, Some(&[0x22u8; 32])).is_err(), "неверный KEK не подходит");
    assert_eq!(NodeIdentity::load_or_create_with_kek(&d, Some(&kek)).unwrap().node_id, plain.node_id);
}
