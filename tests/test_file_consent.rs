//! Защита получателя файлов: согласие (files.accept = known), квоты места и одновременных приёмов,
//! пределы ждущих предложений; квоты DHT-хранилища и таблицы маршрутов ретранслятора.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use p2p_overlay::app::files::{FileOffer, FileStore, Quota, MAX_INCOMING_PER_PEER, MAX_PENDING_PER_PEER};
use p2p_overlay::experiment::net::api;
use p2p_overlay::types::NodeId;

fn offer(id: u8, size: u64) -> FileOffer {
    FileOffer { transfer_id: vec![id; 16], name: format!("f{id}.bin"), size, sha256: vec![0; 32],
                block_size: 40960, blocks: size.div_ceil(40960).max(1) as u32 }
}

fn store(tag: &str, quota: Quota) -> (FileStore, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("p2p-consent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    (FileStore::new(&d, 64 << 20, 60_000).with_quota(quota), d)
}

#[test]
fn quota_on_total_size_rejects_offer() {
    let (mut s, d) = store("quota", Quota { max_total_bytes: 3 << 20, min_free_bytes: 0 });
    let a = NodeId([1; 32]);
    s.on_offer(a, offer(1, 2 << 20), false).unwrap();
    let e = s.check_offer(NodeId([2; 32]), &offer(2, 2 << 20), false).unwrap_err().to_string();
    assert!(e.contains("квота"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn concurrent_incoming_limited_per_peer() {
    let (mut s, d) = store("conc", Quota::default());
    let a = NodeId([1; 32]);
    for i in 0..MAX_INCOMING_PER_PEER as u8 { s.on_offer(a, offer(i + 1, 1000), false).unwrap(); }
    assert!(s.check_offer(a, &offer(50, 1000), false).is_err(), "третий приём от того же узла");
    assert!(s.check_offer(NodeId([2; 32]), &offer(51, 1000), false).is_ok(), "другой узел не задет");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn pending_offers_limited_and_expire() {
    let (mut s, d) = store("pend", Quota::default());
    let a = NodeId([1; 32]);
    for i in 0..MAX_PENDING_PER_PEER as u8 { s.hold(a, offer(i + 1, 1000)).unwrap(); }
    assert!(s.hold(a, offer(60, 1000)).is_err(), "лишнее предложение отклоняется");
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(s.expire_pending(10).len(), MAX_PENDING_PER_PEER, "без ответа — отказ по тайм-ауту");
    let _ = std::fs::remove_dir_all(d);
}

// ── живые узлы: согласие на приём ─────────────────────────────────────────

struct Proc(Child);
impl Drop for Proc { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn node(i: u16, boot: Option<u16>) -> (Proc, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("p2p-consent-n{i}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &(8100 + i).to_string(), "--gui-port", &(8120 + i).to_string(),
            "--state-dir", &d.display().to_string(), "--metrics-dir", &d.join("m").display().to_string(),
            "--mailbox-poll", "0", "--file-accept", "known"]);
    if let Some(b) = boot { c.args(["-b", &format!("127.0.0.1:{b}")]); }
    (Proc(c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("p2p-node")), d)
}

async fn transfer(gui: u16, name: &str) -> serde_json::Value {
    for _ in 0..100 {
        let v = api(gui, "GET", "/api/files", &[], 5).await.unwrap();
        if let Some(t) = v["transfers"].as_array().and_then(|a| a.iter().find(|t| t["name"] == name && t["dir"] == "in").cloned()) {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("на :{gui} нет передачи {name}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stranger_file_waits_for_consent_known_peer_is_automatic() {
    let mut n = vec![node(0, None)];
    for _ in 0..50 { if api(8120, "GET", "/api/status", &[], 1).await.is_ok() { break; } tokio::time::sleep(Duration::from_millis(100)).await; }
    n.extend((1..5).map(|i| node(i, Some(8100))));
    let mut ids = vec![];
    for i in 0..5u16 {
        let mut id = String::new();
        for _ in 0..100 {
            if let Ok(v) = api(8120 + i, "GET", "/api/status", &[], 1).await { id = v["node_id"].as_str().unwrap().into(); break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ids.push(id);
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    let src = n[0].1.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (f, len) in [("a.bin", 300_000usize), ("b.bin", 200_000), ("c.bin", 100_000)] {
        std::fs::write(src.join(f), vec![7u8; len]).unwrap();
    }
    let path = |f: &str| src.join(f).display().to_string();

    // незнакомец: файл ждёт решения; отказ получателя доходит до отправителя
    let (pa, t4) = (path("a.bin"), ids[4].clone());
    let send = tokio::spawn(async move { api(8120, "POST", "/api/send_file", &[("target", &t4), ("path", &pa)], 120).await.unwrap() });
    let t = transfer(8124, "a.bin").await;
    assert_eq!(t["status"], "offered", "{t}");
    assert!(!n[4].1.join("downloads").join(&ids[0][..16]).join("a.bin").exists(), "до согласия на диск ничего не пишется");
    assert!(t["warning"].as_str().unwrap_or("").contains("не из ваших контактов"), "причина в предложении: {t}");
    let tid = t["transfer_id"].as_str().unwrap().to_owned();
    assert_eq!(api(8124, "POST", "/api/file/reject", &[("transfer_id", &tid)], 10).await.unwrap()["ok"], true);
    let r = send.await.unwrap();
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"].as_str().unwrap_or("").contains("отклонил"), "{r}");

    // незнакомец: пользователь принял — файл доставлен
    let (pb, t4) = (path("b.bin"), ids[4].clone());
    let send = tokio::spawn(async move { api(8120, "POST", "/api/send_file", &[("target", &t4), ("path", &pb)], 120).await.unwrap() });
    let t = transfer(8124, "b.bin").await;
    assert_eq!(t["status"], "offered");
    api(8124, "POST", "/api/file/accept", &[("transfer_id", t["transfer_id"].as_str().unwrap())], 10).await.unwrap();
    let r = send.await.unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(std::fs::read(n[4].1.join("downloads").join(&ids[0][..16]).join("b.bin")).unwrap().len(), 200_000, "в папке отправителя");

    // знакомый (получатель сам писал отправителю) — маленький файл принимается сам
    let w = api(8124, "POST", "/api/send", &[("target", &ids[0]), ("text", "привет")], 30).await.unwrap();
    assert_eq!(w["delivered"], true, "{w}");
    let r = api(8120, "POST", "/api/send_file", &[("target", &ids[4]), ("path", &path("c.bin"))], 60).await.unwrap();
    assert_eq!(r["ok"], true, "знакомому — без вопроса: {r}");

    // но исполняемый файл со скрытыми символами в имени — всегда с вопросом и предупреждением
    let spoof = src.join("счёт_\u{202e}fdp.exe");
    std::fs::write(&spoof, b"MZ fake").unwrap();
    let (ps, t4) = (spoof.display().to_string(), ids[4].clone());
    let send = tokio::spawn(async move { api(8120, "POST", "/api/send_file", &[("target", &t4), ("path", &ps)], 120).await.unwrap() });
    let t = transfer(8124, "счёт_\u{202e}fdp.exe").await;
    assert_eq!(t["status"], "offered", "даже от знакомого: {t}");
    assert_eq!(t["ext"], "exe");
    assert!(t["warning"].as_str().unwrap_or("").contains("U+202E"), "{t}");
    api(8124, "POST", "/api/file/reject", &[("transfer_id", t["transfer_id"].as_str().unwrap())], 10).await.unwrap();
    let _ = send.await;
    drop(n);
}

// ── квоты DHT-хранилища и таблицы маршрутов ретранслятора ─────────────────

#[tokio::test]
async fn dht_store_quota_per_publisher() {
    use p2p_overlay::dht::storage::{DhtRecord, DhtStore, RecordKey};
    let s = DhtStore::new(1024);
    let a = NodeId([1; 32]);
    let ok = futures_count(&s, a, 100).await;
    assert_eq!(ok, 64, "один публикатор занимает не больше 1/16 хранилища");
    let b = DhtRecord::new(RecordKey::from_str("other-0"), vec![1], NodeId([2; 32]), 60);
    assert!(s.store(b).await, "другой публикатор по-прежнему пишет");

    async fn futures_count(s: &DhtStore, p: NodeId, n: usize) -> usize {
        let mut ok = 0;
        for i in 0..n {
            if s.store(DhtRecord::new(RecordKey::from_str(&format!("spam-{i}")), vec![0; 10], p, 60)).await { ok += 1; }
        }
        ok
    }
}

#[tokio::test]
async fn relay_routes_quota_per_initiator() {
    use p2p_overlay::tunnel::routes::{TunnelRoutes, MAX_ROUTES_PER_INITIATOR};
    let r = TunnelRoutes::new();
    let addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    let a = NodeId([1; 32]);
    let mut ok = 0;
    for i in 0..(MAX_ROUTES_PER_INITIATOR as u32 + 10) {
        let mut tid = [0u8; 16];
        tid[..4].copy_from_slice(&i.to_be_bytes());
        if r.insert_for(a, tid, addr, None, 60).await { ok += 1; }
    }
    assert_eq!(ok, MAX_ROUTES_PER_INITIATOR);
    assert!(r.insert_for(NodeId([2; 32]), [0xff; 16], addr, None, 60).await, "другой инициатор не задет");
}

#[test]
fn quota_applies_only_to_automatic_acceptance() {
    let (mut s, d) = store("byuser", Quota { max_total_bytes: 3 << 20, min_free_bytes: 0 });
    s.on_offer(NodeId([1; 32]), offer(1, 2 << 20), false).unwrap();
    assert!(s.check_offer(NodeId([2; 32]), &offer(2, 2 << 20), false).is_err(), "сам — не больше квоты");
    assert!(s.check_offer(NodeId([2; 32]), &offer(2, 2 << 20), true).is_ok(), "принятый пользователем — только свободное место");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn human_sizes_and_peer_folder() {
    use p2p_overlay::app::files::{human_bytes, peer_dir};
    assert_eq!(human_bytes(3 * 1024 * 1024 * 1024 + 200 * 1024 * 1024), "3,2 ГиБ");
    assert_eq!(human_bytes(20 * 1024 * 1024), "20,0 МиБ");
    assert_eq!(peer_dir(&NodeId([0xab; 32])), "abababababababab");
}

#[test]
fn very_large_files_go_to_separate_subfolder() {
    use p2p_overlay::app::files::{LARGE_DIR, LARGE_FILE_BYTES};
    let (s, d) = store("large", Quota::default());
    let a = NodeId([0xcd; 32]);
    assert_eq!(s.target_dir(&a, LARGE_FILE_BYTES), d.join("downloads/cdcdcdcdcdcdcdcd"), "ровно 2 ГиБ — обычная папка");
    assert_eq!(s.target_dir(&a, LARGE_FILE_BYTES + 1), d.join("downloads/cdcdcdcdcdcdcdcd").join(LARGE_DIR));
}
