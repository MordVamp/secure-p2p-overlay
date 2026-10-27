//! §12: max_hops, защита от петель на relay, уникальность tunnel_id.

use std::sync::Arc;
use bytes::Bytes;
use tokio::{net::TcpListener, sync::Mutex};
use p2p_overlay::config::NodeConfig;
use p2p_overlay::dht::routing_table::RoutingTable;
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::protocol::payload::{self, TunnelBuildPayload, TunnelDataPayload};
use p2p_overlay::rpc::{PendingRpc, RpcHandler};
use p2p_overlay::transport::connection::FramedStream;
use p2p_overlay::transport::framing::{Frame, FrameFlags, MsgType};
use p2p_overlay::tunnel::routes::TunnelRoutes;
use p2p_overlay::types::Contact;

fn ident(tag: &str) -> NodeIdentity {
    let d = std::env::temp_dir().join(format!("p2p-loop-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    NodeIdentity::load_or_create(&d).unwrap()
}
fn contact(id: &NodeIdentity, port: u16) -> Contact {
    Contact::new(id.node_id, id.public_key_bytes().to_vec(), format!("127.0.0.1:{port}").parse().unwrap())
}
fn relay(tag: &str) -> (RpcHandler, Arc<TunnelRoutes>, Contact) {
    let id = ident(tag);
    let me = contact(&id, 7400);
    let routes = Arc::new(TunnelRoutes::new());
    let h = RpcHandler::new(
        me.clone(), Arc::new(Mutex::new(RoutingTable::new(me.node_id, 3))),
        Arc::new(PendingRpc::new()), Arc::new(NodeConfig::default()),
    ).with_services(Arc::new(Default::default()), routes.clone(), None);
    (h, routes, me)
}
fn frame(t: MsgType, body: Vec<u8>) -> Frame {
    Frame { version: p2p_overlay::protocol::PROTOCOL_VERSION, msg_type: t, flags: FrameFlags::empty(),
            request_id: [7; 16], payload: Bytes::from(body) }
}
fn build(init: &Contact, next: Option<Contact>, idx: usize, total: usize, tid: [u8; 16]) -> Frame {
    frame(MsgType::TunnelBuild, payload::encode(&TunnelBuildPayload {
        tunnel_id: tid.to_vec(), initiator: init.clone(), hop_index: idx, total_hops: total,
        ttl_seconds: 60, timestamp_ms: 0, next_hop: next,
    }).unwrap())
}

#[tokio::test]
async fn valid_build_accepted() {
    let (h, _, _) = relay("r0");
    let (init, nxt) = (contact(&ident("i0"), 7401), contact(&ident("n0"), 7402));
    assert!(h.handle_tunnel_build(&build(&init, Some(nxt), 0, 2, [1; 16])).await.is_ok());
}

#[tokio::test]
async fn total_hops_above_max_rejected() {
    let (h, _, _) = relay("r1");
    let max = NodeConfig::default().tunnel.max_hops;
    let (init, nxt) = (contact(&ident("i1"), 7401), contact(&ident("n1"), 7402));
    assert!(h.handle_tunnel_build(&build(&init, Some(nxt), 0, max + 1, [2; 16])).await.is_err());
}

#[tokio::test]
async fn zero_total_hops_and_bad_index_rejected() {
    let (h, _, _) = relay("r2");
    let (init, nxt) = (contact(&ident("i2"), 7401), contact(&ident("n2"), 7402));
    assert!(h.handle_tunnel_build(&build(&init, Some(nxt.clone()), 0, 0, [3; 16])).await.is_err());
    assert!(h.handle_tunnel_build(&build(&init, Some(nxt), 3, 3, [4; 16])).await.is_err(), "hop_index >= total_hops");
}

#[tokio::test]
async fn next_hop_equal_to_initiator_rejected() {
    let (h, _, _) = relay("r3");
    let init = contact(&ident("i3"), 7401);
    assert!(h.handle_tunnel_build(&build(&init, Some(init.clone()), 0, 2, [5; 16])).await.is_err(), "петля на инициатора");
}

#[tokio::test]
async fn next_hop_equal_to_self_rejected() {
    let (h, _, me) = relay("r4");
    let init = contact(&ident("i4"), 7401);
    assert!(h.handle_tunnel_build(&build(&init, Some(me), 0, 2, [6; 16])).await.is_err());
}

#[tokio::test]
async fn duplicate_tunnel_id_rejected() {
    let (h, _, _) = relay("r5");
    let (init, n1, n2) = (contact(&ident("i5"), 7401), contact(&ident("n5"), 7402), contact(&ident("m5"), 7403));
    assert!(h.handle_tunnel_build(&build(&init, Some(n1), 0, 2, [8; 16])).await.is_ok());
    assert!(h.handle_tunnel_build(&build(&init, Some(n2), 0, 2, [8; 16])).await.is_err(),
            "повторный BUILD не должен перенаправлять существующий туннель");
}

#[tokio::test]
async fn relay_drops_data_when_hops_exhausted() {
    let (h, routes, _) = relay("r6");
    routes.insert([9; 16], "127.0.0.1:1".parse().unwrap(), 60).await; // петля/неважно куда
    let d = payload::encode(&TunnelDataPayload { tunnel_id: vec![9; 16], seq: 0, data: b"x".to_vec(), hops_left: 0 }).unwrap();
    assert!(h.handle_tunnel_data(&frame(MsgType::TunnelData, d)).await.is_err(), "hops_left=0 → отбросить");
}

#[tokio::test]
async fn data_without_hops_field_is_not_forwarded() {
    // кадр старого формата (нет hops_left → 0) не должен пересылаться
    let (h, routes, _) = relay("r7");
    routes.insert([10; 16], "127.0.0.1:1".parse().unwrap(), 60).await;
    #[derive(serde::Serialize)]
    struct Old { tunnel_id: Vec<u8>, seq: u64, data: Vec<u8> }
    let d = rmp_serde::to_vec_named(&Old { tunnel_id: vec![10; 16], seq: 0, data: vec![1] }).unwrap();
    assert!(h.handle_tunnel_data(&frame(MsgType::TunnelData, d)).await.is_err());
}

#[tokio::test]
async fn relay_decrements_hops_left_when_forwarding() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let next = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut fs = FramedStream::new(s, 3000);
        let f = fs.recv().await.unwrap();
        let p: TunnelDataPayload = payload::decode(&f.payload).unwrap();
        let ack = payload::encode(&p2p_overlay::protocol::payload::TunnelAckPayload {
            tunnel_id: p.tunnel_id.clone(), seq: p.seq, rtt_ms: 0 }).unwrap();
        fs.send(&Frame::new_response(MsgType::TunnelAck, f.request_id, Bytes::from(ack))).await.unwrap();
        p.hops_left
    });
    let (h, routes, _) = relay("r8");
    routes.insert([11; 16], addr, 60).await;
    let d = payload::encode(&TunnelDataPayload { tunnel_id: vec![11; 16], seq: 0, data: b"hi".to_vec(), hops_left: 4 }).unwrap();
    assert!(h.handle_tunnel_data(&frame(MsgType::TunnelData, d)).await.is_ok());
    assert_eq!(next.await.unwrap(), 3, "relay должен уменьшить hops_left на 1");
}

#[tokio::test]
async fn routing_cycle_terminates() {
    // Два relay с зацикленными таблицами: данные не должны ходить бесконечно.
    // A.next = B, B.next = A (у обоих прослушивающий сервер на реальном сокете).
    let (ha, ra, _) = relay("cA");
    let (hb, rb, _) = relay("cB");
    let (ha, hb) = (Arc::new(ha), Arc::new(hb));
    let la = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let lb = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (aa, ab) = (la.local_addr().unwrap(), lb.local_addr().unwrap());
    ra.insert([12; 16], ab, 60).await; // A → B
    rb.insert([12; 16], aa, 60).await; // B → A
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for (l, h) in [(la, ha.clone()), (lb, hb.clone())] {
        let hits = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l.accept().await else { break };
                let (h, hits) = (h.clone(), hits.clone());
                tokio::spawn(async move {
                    let mut fs = FramedStream::new(s, 5000);
                    if let Ok(f) = fs.recv().await {
                        hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let resp = match h.handle_tunnel_data(&f).await {
                            Ok(r) => r,
                            Err(_) => Frame::new_response(MsgType::Error, f.request_id, Bytes::new()),
                        };
                        let _ = fs.send(&resp).await;
                    }
                });
            }
        });
    }
    let hops = 5u8;
    let d = payload::encode(&TunnelDataPayload { tunnel_id: vec![12; 16], seq: 0, data: b"loop".to_vec(), hops_left: hops }).unwrap();
    let r = tokio::time::timeout(std::time::Duration::from_secs(10), ha.handle_tunnel_data(&frame(MsgType::TunnelData, d))).await;
    assert!(r.is_ok(), "цикл должен завершиться, а не зависнуть");
    let n = hits.load(std::sync::atomic::Ordering::SeqCst);
    assert!(n <= hops as usize, "число пересылок {n} ограничено hops_left={hops}");
}
