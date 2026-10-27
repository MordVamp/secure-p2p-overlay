//! APP_ACK (ТЗ §13.1): подтверждение доставки; подделка/чужой отправитель/повтор не засчитываются.

use std::sync::Arc;
use p2p_overlay::app::message::AppMessage;
use p2p_overlay::app::messenger::Messenger;
use p2p_overlay::config::NodeConfig;
use p2p_overlay::dht::node::DhtNode;
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::protocol::payload::encode;
use p2p_overlay::security::e2e::seal;
use p2p_overlay::tunnel::manager::TunnelManager;
use p2p_overlay::types::Contact;

fn ident(tag: &str) -> NodeIdentity {
    let d = std::env::temp_dir().join(format!("p2p-ack-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    NodeIdentity::load_or_create(&d).unwrap()
}

fn messenger(id: &Arc<NodeIdentity>) -> Arc<Messenger> {
    let cfg = Arc::new(NodeConfig::default());
    let me = Contact::new(id.node_id, id.public_key_bytes().to_vec(), "127.0.0.1:7500".parse().unwrap());
    let dht = Arc::new(DhtNode::new(me.clone(), cfg.clone()));
    let tm = Arc::new(TunnelManager::new(me, dht.routing.clone(), cfg.clone()));
    Arc::new(Messenger::new(id.clone(), dht.clone(), tm.clone(), tm.forwarder.clone(), dht.metrics.clone(), cfg))
}

/// Зашифрованное для `to` сообщение, подписанное `from`.
fn wire(from: &NodeIdentity, to: &NodeIdentity, mut m: AppMessage, tamper_sig: bool) -> Vec<u8> {
    m.sender_pubkey = from.public_key_bytes().to_vec();
    m.signature = from.sign(&m.signing_bytes()).to_bytes().to_vec();
    if tamper_sig { m.signature[0] ^= 1; }
    seal(&to.node_id, &to.public_key_bytes(), &encode(&m).unwrap()).unwrap()
}

#[tokio::test]
async fn valid_ack_from_target_wakes_sender() {
    let (a, b) = (Arc::new(ident("a1")), ident("b1"));
    let ma = messenger(&a);
    let id = vec![1u8; 16];
    let mut rx = ma.register_ack_wait(id.clone(), b.node_id).await;
    ma.handle_incoming(wire(&b, &a, AppMessage::new_ack(b.node_id, a.node_id, &id), false)).await.unwrap();
    assert!(rx.try_recv().is_ok(), "отправитель должен получить подтверждение");
}

#[tokio::test]
async fn ack_from_wrong_node_is_ignored() {
    let (a, b, eve) = (Arc::new(ident("a2")), ident("b2"), ident("e2"));
    let ma = messenger(&a);
    let id = vec![2u8; 16];
    let mut rx = ma.register_ack_wait(id.clone(), b.node_id).await;
    // Ева валидно подписывает и шифрует ACK на чужое сообщение
    ma.handle_incoming(wire(&eve, &a, AppMessage::new_ack(eve.node_id, a.node_id, &id), false)).await.unwrap();
    assert!(rx.try_recv().is_err(), "ACK не от адресата не засчитывается");
}

#[tokio::test]
async fn forged_ack_signature_rejected() {
    let (a, b) = (Arc::new(ident("a3")), ident("b3"));
    let ma = messenger(&a);
    let id = vec![3u8; 16];
    let mut rx = ma.register_ack_wait(id.clone(), b.node_id).await;
    let r = ma.handle_incoming(wire(&b, &a, AppMessage::new_ack(b.node_id, a.node_id, &id), true)).await;
    assert!(r.is_err(), "ACK с неверной подписью отвергается");
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn replayed_ack_rejected() {
    let (a, b) = (Arc::new(ident("a4")), ident("b4"));
    let ma = messenger(&a);
    let id = vec![4u8; 16];
    let _rx = ma.register_ack_wait(id.clone(), b.node_id).await;
    let w = wire(&b, &a, AppMessage::new_ack(b.node_id, a.node_id, &id), false);
    ma.handle_incoming(w.clone()).await.unwrap();
    assert!(ma.handle_incoming(w).await.is_err(), "повтор того же ACK отвергается (anti-replay)");
}

#[tokio::test]
async fn unsolicited_ack_does_not_crash_or_land_in_inbox() {
    let (a, b) = (Arc::new(ident("a5")), ident("b5"));
    let ma = messenger(&a);
    ma.handle_incoming(wire(&b, &a, AppMessage::new_ack(b.node_id, a.node_id, &[9u8; 16]), false)).await.unwrap();
    assert!(ma.recent().await.is_empty(), "ACK не показывается как сообщение");
}

#[tokio::test]
async fn text_goes_to_inbox() {
    let (a, b) = (Arc::new(ident("a6")), ident("b6"));
    let ma = messenger(&a);
    ma.handle_incoming(wire(&b, &a, AppMessage::new_text(b.node_id, a.node_id, "hi"), false)).await.unwrap();
    let inbox = ma.recent().await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text(), Some("hi"));
}

#[tokio::test]
async fn send_acked_fails_fast_without_route() {
    let (a, b) = (Arc::new(ident("a7")), ident("b7"));
    let ma = messenger(&a);
    let r = ma.send_text_acked(b.node_id, "x", std::time::Duration::from_millis(300)).await;
    assert!(r.is_err(), "без маршрута доставка не может быть подтверждена");
}

#[tokio::test]
async fn route_diag_without_tunnel() {
    let (a, b) = (Arc::new(ident("a8")), ident("b8"));
    let d = messenger(&a).route_diag(&b.node_id).await;
    assert_eq!(d["in_routing_table"], false);
    assert!(d["tunnel"].is_null());
}

/// Регрессия: получатель, найденный lookup'ом, но не удержанный таблицей (малый k),
/// должен использоваться как есть, а не давать «target unknown».
#[tokio::test]
async fn builder_uses_lookup_hint_when_table_lacks_target() {
    use p2p_overlay::dht::routing_table::RoutingTable;
    use p2p_overlay::tunnel::builder::TunnelBuilder;
    use tokio::sync::Mutex;

    let (me, target) = (ident("h0"), ident("h1"));
    let own = Contact::new(me.node_id, me.public_key_bytes().to_vec(), "127.0.0.1:7500".parse().unwrap());
    let tgt = Contact::new(target.node_id, target.public_key_bytes().to_vec(), "127.0.0.1:1".parse().unwrap());
    let mut rt = RoutingTable::new(me.node_id, 3);
    for (i, tag) in ["r1", "r2"].iter().enumerate() {
        let r = ident(tag);
        rt.update(Contact::new(r.node_id, r.public_key_bytes().to_vec(), format!("127.0.0.1:{}", 2 + i).parse().unwrap()));
    }
    let b = TunnelBuilder::new(own, Arc::new(Mutex::new(rt)), Arc::new(NodeConfig::default()));

    let e1 = b.build(&target.node_id).await.err().unwrap().to_string();
    assert!(e1.contains("Failed to build"), "{e1}");
    // без подсказки причина — «unknown», с подсказкой дело доходит до реального соединения с relay
    let e2 = format!("{:#}", b.build_with(&target.node_id, Some(&tgt)).await.err().unwrap());
    assert!(!e2.contains("unknown"), "с hint получатель известен: {e2}");

    // подсказка с чужой парой NodeID/pubkey игнорируется
    let bad = Contact::new(target.node_id, me.public_key_bytes().to_vec(), "127.0.0.1:1".parse().unwrap());
    assert!(b.build_with(&target.node_id, Some(&bad)).await.is_err());
}
