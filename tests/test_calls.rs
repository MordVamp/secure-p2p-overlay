//! Звонки (шаги 2–4 плана): ключ звонка, шифрование медиакадров, окно против повторов,
//! сигнализация и медиатранспорт TUNNEL_MEDIA на живых узлах.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use p2p_overlay::app::calls::{ephemeral, CallKeys, ReplayWindow, RxStats};
use p2p_overlay::experiment::net::api;
use p2p_overlay::types::NodeId;

fn keys() -> (CallKeys, CallKeys, Vec<u8>) {
    let id = vec![7u8; 16];
    let (caller, callee) = (NodeId([1; 32]), NodeId([2; 32]));
    let (cs, cp) = ephemeral();
    let (es, ep) = ephemeral();
    let a = CallKeys::derive(&id, &caller, &callee, &cp, &ep, &cs, true).unwrap();
    let b = CallKeys::derive(&id, &caller, &callee, &cp, &ep, &es, false).unwrap();
    (a, b, id)
}

#[test]
fn both_sides_derive_matching_directional_keys() {
    let (a, b, id) = keys();
    let p = a.seal(&id, 5, 123, b"voice").unwrap();
    assert_eq!(b.open(&p).unwrap(), (5, 123, b"voice".to_vec()), "звонящий → вызываемый");
    let q = b.seal(&id, 9, 456, b"back").unwrap();
    assert_eq!(a.open(&q).unwrap(), (9, 456, b"back".to_vec()), "обратное направление");
    // свой же кадр, вернувшийся обратно, не открывается: ключи направлений разные
    assert!(a.open(&p).is_err());
}

#[test]
fn tampered_or_foreign_media_rejected() {
    let (a, b, id) = keys();
    let mut p = a.seal(&id, 1, 0, b"voice").unwrap();
    let last = p.len() - 1;
    p[last] ^= 1;
    assert!(b.open(&p).is_err(), "изменённый шифртекст");
    let mut q = a.seal(&id, 1, 0, b"voice").unwrap();
    q[17] ^= 1;                                   // номер кадра входит в AAD и nonce
    assert!(b.open(&q).is_err(), "подменённый номер кадра");
    let (_, c, _) = keys();                       // другой звонок
    assert!(c.open(&a.seal(&id, 2, 0, b"x").unwrap()).is_err(), "чужой ключ звонка");
}

#[test]
fn weak_ephemeral_key_rejected() {
    let (s, _) = ephemeral();
    let r = CallKeys::derive(&[0; 16], &NodeId([1; 32]), &NodeId([2; 32]), &[0; 32], &[0; 32], &s, false);
    assert!(r.is_err(), "нулевой ключ собеседника даёт нулевой общий секрет");
}

#[test]
fn replay_window_accepts_late_once_and_rejects_old() {
    let mut w = ReplayWindow::default();
    assert!(w.accept(10));
    assert!(!w.accept(10), "повтор");
    assert!(w.accept(8), "опоздавший, но в окне");
    assert!(!w.accept(8));
    assert!(w.accept(100));
    assert!(!w.accept(36), "старше окна в 64 номера");
    assert!(w.accept(37));
}

#[test]
fn rx_stats_count_loss_and_reorder() {
    let mut s = RxStats::default();
    for (seq, t) in [(0u64, 0u64), (1, 20_000), (3, 60_000), (2, 40_000)] { s.record(seq, t, t + 1000, 80); }
    assert_eq!(s.packets, 4);
    assert_eq!(s.lost(), 0);
    assert_eq!(s.reordered, 1);
    s.record(9, 180_000, 181_000, 80);
    assert_eq!(s.lost(), 5, "6, 4, 5, 7, 8 не пришли");
}

// ── живые узлы ──────────────────────────────────────────────────────────────

struct Proc(Child);
impl Drop for Proc { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn node(i: u16, boot: Option<u16>) -> Proc {
    let d = std::env::temp_dir().join(format!("p2p-call-{i}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &(7900 + i).to_string(), "--gui-port", &(7920 + i).to_string(),
            "--state-dir", &d.display().to_string(), "--metrics-dir", &d.join("m").display().to_string(),
            "--mailbox-poll", "0"]);
    if let Some(b) = boot { c.args(["-b", &format!("127.0.0.1:{b}")]); }
    Proc(c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("p2p-node"))
}

async fn call(gui: u16, id: &str) -> serde_json::Value {
    let v = api(gui, "GET", "/api/calls", &[], 5).await.unwrap();
    v["calls"].as_array().unwrap().iter().find(|c| c["call_id"] == id).cloned().unwrap_or_default()
}

async fn wait_state(gui: u16, id: &str, state: &str) -> serde_json::Value {
    for _ in 0..100 {
        let c = call(gui, id).await;
        if c["state"] == state { return c; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("звонок {id} на :{gui} не перешёл в {state}: {}", call(gui, id).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn call_signaling_keys_and_media_over_tunnels() {
    let mut n = vec![node(0, None)];
    for _ in 0..50 { if api(7920, "GET", "/api/status", &[], 1).await.is_ok() { break; } tokio::time::sleep(Duration::from_millis(100)).await; }
    n.extend((1..5).map(|i| node(i, Some(7900))));
    let mut ids = vec![];
    for i in 0..5u16 {
        let mut id = String::new();
        for _ in 0..100 {
            if let Ok(v) = api(7920 + i, "GET", "/api/status", &[], 1).await { id = v["node_id"].as_str().unwrap().into(); break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ids.push(id);
    }
    tokio::time::sleep(Duration::from_secs(4)).await;

    // вызов 0 → 4: у вызываемого — входящий, у звонящего — ожидание ответа
    let r = api(7920, "POST", "/api/call/start", &[("target", &ids[4])], 30).await.unwrap();
    assert_eq!(r["ok"], true, "{r}");
    let cid = r["call_id"].as_str().unwrap().to_owned();
    let inc = wait_state(7924, &cid, "ringing").await;
    assert_eq!(inc["outgoing"], false);
    assert_eq!(inc["peer"], ids[0]);

    // занято: третий узел звонит вызываемому, пока идёт вызов
    let busy = api(7921, "POST", "/api/call/start", &[("target", &ids[4])], 30).await.unwrap();
    let bid = busy["call_id"].as_str().unwrap().to_owned();
    let b = wait_state(7921, &bid, "ended").await;
    assert_eq!(b["end_reason"], "busy");

    // ответ: у обоих — активный звонок с ключами
    assert_eq!(api(7924, "POST", "/api/call/accept", &[("call_id", &cid)], 30).await.unwrap()["ok"], true);
    let out = wait_state(7920, &cid, "active").await;
    assert!(out["setup_ms"].as_u64().is_some());

    // медиа в обе стороны: TUNNEL_MEDIA без подтверждений, шифрование ключом звонка
    let m = api(7920, "POST", "/api/call/media", &[("call_id", &cid), ("count", "100"), ("interval_ms", "10"), ("size", "80")], 60).await.unwrap();
    assert_eq!(m["report"]["sent"], 100, "{m}");
    let m = api(7924, "POST", "/api/call/media", &[("call_id", &cid), ("count", "50"), ("interval_ms", "10"), ("size", "80")], 60).await.unwrap();
    assert_eq!(m["report"]["sent"], 50, "{m}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let rx = call(7924, &cid).await;
    assert!(rx["rx"]["packets"].as_u64().unwrap() >= 95, "вызываемый принял медиа: {rx}");
    assert_eq!(rx["rx"]["rejected"], 0);
    assert!(rx["one_way_ms"]["median"].as_f64().is_some());
    let tx = call(7920, &cid).await;
    assert!(tx["rx"]["packets"].as_u64().unwrap() >= 45, "звонящий принял ответный поток: {tx}");

    // завершение: у обоих ended, в истории — запись о звонке
    assert_eq!(api(7920, "POST", "/api/call/hangup", &[("call_id", &cid)], 30).await.unwrap()["ok"], true);
    let e = wait_state(7924, &cid, "ended").await;
    assert_eq!(e["end_reason"], "hangup");
    let h = api(7924, "GET", "/api/history", &[("limit", "20")], 5).await.unwrap();
    assert!(h["entries"].as_array().unwrap().iter().any(|x| x["message_id"] == cid && x["status"] == "call_ended"), "{h}");
    drop(n);
}
