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

fn node(i: u16, boot: Option<u16>) -> Proc { node_at(7900, 7920, i, boot) }

fn node_at(p2p: u16, gui: u16, i: u16, boot: Option<u16>) -> Proc {
    let d = std::env::temp_dir().join(format!("p2p-call-{}-{}", p2p + i, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &(p2p + i).to_string(), "--gui-port", &(gui + i).to_string(),
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

// ── звук: Opus, джиттер-буфер, смешивание; групповой звонок; локальный аудиоканал ─────────────

async fn start_net(p2p: u16, gui: u16, n: u16) -> (Vec<Proc>, Vec<String>) {
    let mut v = vec![node_at(p2p, gui, 0, None)];
    for _ in 0..50 { if api(gui, "GET", "/api/status", &[], 1).await.is_ok() { break; } tokio::time::sleep(Duration::from_millis(100)).await; }
    v.extend((1..n).map(|i| node_at(p2p, gui, i, Some(p2p))));
    let mut ids = vec![];
    for i in 0..n {
        let mut id = String::new();
        for _ in 0..100 {
            if let Ok(s) = api(gui + i, "GET", "/api/status", &[], 1).await { id = s["node_id"].as_str().unwrap().into(); break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ids.push(id);
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    (v, ids)
}

async fn session(gui: u16, sid: &str) -> serde_json::Value {
    let v = api(gui, "GET", "/api/calls", &[], 5).await.unwrap();
    v["sessions"].as_array().unwrap().iter().find(|c| c["id"] == sid).cloned().unwrap_or_default()
}

async fn wait_session(gui: u16, sid: &str, f: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    for _ in 0..150 {
        let s = session(gui, sid).await;
        if f(&s) { return s; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("сеанс {sid} на :{gui}: {}", session(gui, sid).await);
}

fn active_peers(s: &serde_json::Value) -> usize {
    s["peers"].as_array().map_or(0, |p| p.iter().filter(|x| x["state"] == "active").count())
}

async fn tone(gui: u16, freq: &str, secs: &str) -> serde_json::Value {
    api(gui, "POST", "/api/call/tone", &[("freq", freq), ("seconds", secs), ("listen", "440,554,660,880")], 60).await.unwrap()["report"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn voice_one_to_one_group_mesh_and_local_audio_link() {
    const P: u16 = 7960;
    const G: u16 = 7980;
    let (n, ids) = start_net(P, G, 6).await;

    // 1:1 — A играет 440 Гц, B — 660 Гц: каждый слышит только собеседника
    let cid = api(G, "POST", "/api/call/start", &[("target", &ids[1])], 30).await.unwrap()["call_id"].as_str().unwrap().to_owned();
    wait_session(G + 1, &cid, |s| s["state"] == "ringing").await;
    assert_eq!(api(G + 1, "POST", "/api/call/accept", &[("call_id", &cid)], 30).await.unwrap()["ok"], true);
    wait_session(G, &cid, |s| s["state"] == "active").await;
    let (a, b) = tokio::join!(tone(G, "440", "4"), tone(G + 1, "660", "4"));
    assert_eq!(a["heard"], serde_json::json!([660.0]), "A слышит B: {a}");
    assert_eq!(b["heard"], serde_json::json!([440.0]), "B слышит A: {b}");
    let s = session(G, &cid).await;
    let q = &s["peers"][0]["quality"];
    assert!(q["audio"]["played"].as_u64().unwrap() > 100, "{s}");
    assert!(q["mos"].as_f64().unwrap() > 4.0, "в LAN без потерь MOS высокий: {q}");
    api(G, "POST", "/api/call/hangup", &[("call_id", &cid)], 30).await.unwrap();
    wait_session(G + 1, &cid, |s| s["state"] == "ended").await;

    // групповой: A (0) зовёт B (1) и C (2); B и C принимают и соединяются между собой сами
    let members = format!("{},{}", ids[1], ids[2]);
    let r = api(G, "POST", "/api/call/start", &[("members", &members), ("name", "Тест")], 30).await.unwrap();
    assert_eq!(r["ok"], true, "{r}");
    let sid = r["call_id"].as_str().unwrap().to_owned();
    for g in [G + 1, G + 2] {
        let s = wait_session(g, &sid, |s| s["state"] == "ringing").await;
        assert_eq!(s["group"]["name"], "Тест");
        assert_eq!(s["outgoing"], false);
    }
    assert_eq!(api(G + 1, "POST", "/api/call/accept", &[("call_id", &sid)], 30).await.unwrap()["ok"], true);
    assert_eq!(api(G + 2, "POST", "/api/call/accept", &[("call_id", &sid)], 30).await.unwrap()["ok"], true);
    for g in [G, G + 1, G + 2] { wait_session(g, &sid, |s| active_peers(s) == 2).await; }
    // занято: четвёртый узел не дозвонится участнику группового звонка
    let busy = api(G + 3, "POST", "/api/call/start", &[("target", &ids[2])], 30).await.unwrap();
    let bid = busy["call_id"].as_str().unwrap().to_owned();
    let b = wait_session(G + 3, &bid, |s| s["state"] == "ended").await;
    assert_eq!(b["end_reason"], "busy");

    // каждый слышит двух других (смешивание в узле)
    let (x, y, z) = tokio::join!(tone(G, "440", "5"), tone(G + 1, "554", "5"), tone(G + 2, "660", "5"));
    assert_eq!(x["heard"], serde_json::json!([554.0, 660.0]), "A: {x}");
    assert_eq!(y["heard"], serde_json::json!([440.0, 660.0]), "B: {y}");
    assert_eq!(z["heard"], serde_json::json!([440.0, 554.0]), "C: {z}");

    // инициатор ушёл — B и C продолжают; ушёл B — у C звонок закончен, запись в истории
    api(G, "POST", "/api/call/hangup", &[("call_id", &sid)], 30).await.unwrap();
    wait_session(G + 1, &sid, |s| s["state"] == "active" && active_peers(s) == 1).await;
    api(G + 1, "POST", "/api/call/hangup", &[("call_id", &sid)], 30).await.unwrap();
    wait_session(G + 2, &sid, |s| s["state"] == "ended").await;
    let h = api(G + 2, "GET", "/api/history", &[("limit", "50")], 5).await.unwrap();
    assert!(h["entries"].as_array().unwrap().iter().any(|e| e["message_id"] == sid.as_str() && e["status"] == "call_ended"
        && e["text"].as_str().unwrap_or("").contains("Групповой")), "{h}");

    // локальный аудиоканал: кадр без верного токена игнорируется, с верным — ответ того же размера
    let a = api(G, "GET", "/api/call/audio", &[], 5).await.unwrap();
    let port = a["port"].as_u64().unwrap() as u16;
    let token = hex::decode(a["token"].as_str().unwrap()).unwrap();
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let frame = |t: &[u8]| { let mut b = b"P2A1".to_vec(); b.extend_from_slice(t); b.extend(std::iter::repeat(0u8).take(640)); b };
    sock.send_to(&frame(&[0u8; 16]), ("127.0.0.1", port)).await.unwrap();
    let mut buf = [0u8; 2048];
    assert!(tokio::time::timeout(Duration::from_millis(500), sock.recv_from(&mut buf)).await.is_err(), "чужой токен — без ответа");
    sock.send_to(&frame(&token), ("127.0.0.1", port)).await.unwrap();
    let (len, _) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await.unwrap().unwrap();
    assert_eq!(len, 4 + 640);
    drop(n);
}
