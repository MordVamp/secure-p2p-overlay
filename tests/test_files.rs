//! §13.2: передача файлов — приёмник (безопасное имя, лимит, дубликаты, подмена, чужой отправитель, тайм-аут)
//! и укладка блока в сегмент туннеля и кадр после подписи и E2E-шифрования.

use p2p_overlay::app::files::{safe_name, BlockOutcome, FileBlock, FileOffer, FileStore, MAX_BLOCK};
use p2p_overlay::app::message::{AppMessage, MessageKind};
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::protocol::payload::{encode, TunnelDataPayload};
use p2p_overlay::types::NodeId;
use sha2::{Digest, Sha256};

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2p-files-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const BS: u32 = 1000;

fn offer(name: &str, data: &[u8]) -> FileOffer {
    FileOffer { transfer_id: vec![7; 16], name: name.into(), size: data.len() as u64, sha256: Sha256::digest(data).to_vec(),
                block_size: BS, blocks: (data.len() as u32).div_ceil(BS).max(1) }
}

fn block(data: &[u8], i: u32) -> FileBlock {
    let s = (i * BS) as usize;
    FileBlock { transfer_id: vec![7; 16], index: i, data: data[s..(s + BS as usize).min(data.len())].to_vec() }
}

fn sample(n: usize) -> Vec<u8> { (0..n).map(|i| (i * 31 % 251) as u8).collect() }

const A: NodeId = NodeId([1; 32]);
const EVE: NodeId = NodeId([2; 32]);

#[test]
fn unsafe_names_rejected() {
    for bad in ["", "..", "../x", "a/b", "a\\b", "c:x", ".hidden", "x\0y", "dir/../etc"] {
        assert!(safe_name(bad).is_err(), "{bad:?} должно отвергаться");
    }
    for ok in ["report.pdf", "фото 1.jpg", "data_2026-11-02.tar.gz"] { assert!(safe_name(ok).is_ok(), "{ok}"); }
}

#[test]
fn complete_transfer_out_of_order_with_duplicate() {
    let d = tmp("ok");
    let data = sample(3500);   // 4 блока, последний неполный
    let mut st = FileStore::new(&d, 1 << 20, 60_000);
    st.on_offer(A, offer("a.bin", &data), false).unwrap();
    for i in [2, 0, 3] { assert_eq!(st.on_block(A, &block(&data, i)).unwrap(), BlockOutcome::Stored); }
    assert_eq!(st.on_block(A, &block(&data, 0)).unwrap(), BlockOutcome::Duplicate, "дубликат игнорируется");
    assert_eq!(st.ack(&[7; 16], 4).unwrap().missing, vec![1]);
    assert_eq!(st.on_block(A, &block(&data, 1)).unwrap(), BlockOutcome::Complete);
    let path = st.finish(&[7; 16]).unwrap();
    assert_eq!(path, d.join("downloads").join(p2p_overlay::app::files::peer_dir(&A)).join("a.bin"), "в папке отправителя");
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(std::fs::read_dir(d.join("downloads/.partial")).unwrap().count(), 0, "неполный файл перенесён");
}

#[test]
fn tampered_block_fails_hash_and_file_is_removed() {
    let d = tmp("tamper");
    let data = sample(2500);
    let mut st = FileStore::new(&d, 1 << 20, 60_000);
    st.on_offer(A, offer("t.bin", &data), false).unwrap();
    let mut bad = block(&data, 1);
    bad.data[10] ^= 0xFF;            // длина та же, содержимое подменено
    st.on_block(A, &block(&data, 0)).unwrap();
    st.on_block(A, &bad).unwrap();
    assert_eq!(st.on_block(A, &block(&data, 2)).unwrap(), BlockOutcome::Complete);
    assert!(st.finish(&[7; 16]).is_err(), "SHA-256 не совпадает");
    assert!(!d.join("downloads").join(p2p_overlay::app::files::peer_dir(&A)).join("t.bin").exists());
    assert_eq!(std::fs::read_dir(d.join("downloads/.partial")).unwrap().count(), 0, "неполный файл удалён");
}

#[test]
fn oversize_and_bad_name_and_inconsistent_offers_rejected() {
    let d = tmp("offer");
    let data = sample(5000);
    let mut st = FileStore::new(&d, 4000, 60_000);
    assert!(st.on_offer(A, offer("big.bin", &data), false).is_err(), "больше лимита");
    let mut st = FileStore::new(&d, 1 << 20, 60_000);
    assert!(st.on_offer(A, offer("../evil", &data), false).is_err(), "путь в имени");
    let mut o = offer("x.bin", &data);
    o.blocks = 99;
    assert!(st.on_offer(A, o, false).is_err(), "число блоков не соответствует размеру");
    let mut o = offer("x.bin", &data);
    o.block_size = (MAX_BLOCK + 1) as u32;
    o.blocks = 1;
    assert!(st.on_offer(A, o, false).is_err(), "блок больше допустимого");
}

#[test]
fn foreign_sender_wrong_index_and_length_rejected() {
    let d = tmp("foreign");
    let data = sample(2500);
    let mut st = FileStore::new(&d, 1 << 20, 60_000);
    st.on_offer(A, offer("f.bin", &data), false).unwrap();
    assert!(st.on_block(EVE, &block(&data, 0)).is_err(), "блок от другого узла");
    let mut b = block(&data, 0);
    b.index = 9;
    assert!(st.on_block(A, &b).is_err(), "индекс вне диапазона");
    let mut b = block(&data, 0);
    b.data.truncate(10);
    assert!(st.on_block(A, &b).is_err(), "неверная длина блока");
    assert_eq!(st.progress(&[7; 16]).unwrap().0, 0, "ни один из отвергнутых блоков не записан");
}

#[test]
fn incomplete_file_removed_after_timeout() {
    let d = tmp("timeout");
    let data = sample(2500);
    let mut st = FileStore::new(&d, 1 << 20, 0);
    st.on_offer(A, offer("slow.bin", &data), false).unwrap();
    st.on_block(A, &block(&data, 0)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_eq!(st.evict_stale().len(), 1);
    assert_eq!(std::fs::read_dir(d.join("downloads/.partial")).unwrap().count(), 0);
    assert!(st.on_block(A, &block(&data, 1)).is_err(), "передача после тайм-аута забыта");
}

#[test]
fn name_collision_gets_suffix() {
    let d = tmp("dup");
    let data = sample(500);
    let mut st = FileStore::new(&d, 1 << 20, 60_000);
    for k in 0..2u8 {
        let mut o = offer("same.txt", &data);
        o.transfer_id = vec![k; 16];
        st.on_offer(A, o, false).unwrap();
        let mut b = block(&data, 0);
        b.transfer_id = vec![k; 16];
        st.on_block(A, &b).unwrap();
        st.finish(&[k; 16]).unwrap();
    }
    let dir = d.join("downloads").join(p2p_overlay::app::files::peer_dir(&A));
    assert!(dir.join("same.txt").exists() && dir.join("same (1).txt").exists());
}

#[test]
fn largest_block_fits_tunnel_segment_and_frame() {
    let d = tmp("size");
    let (a, b) = (NodeIdentity::load_or_create(&d.join("a")).unwrap(), NodeIdentity::load_or_create(&d.join("b")).unwrap());
    let blk = FileBlock { transfer_id: vec![1; 16], index: 1_000_000, data: vec![0xAB; MAX_BLOCK] };
    let mut m = AppMessage::new_kind(a.node_id, b.node_id, MessageKind::FileBlock, encode(&blk).unwrap());
    m.sender_pubkey = a.public_key_bytes().to_vec();
    m.signature = a.sign(&m.signing_bytes()).to_bytes().to_vec();
    let env = p2p_overlay::security::e2e::seal(&b.node_id, &b.public_key_bytes(), &encode(&m).unwrap()).unwrap();
    assert!(env.len() <= p2p_overlay::tunnel::forwarder::MAX_SEGMENT_SIZE, "конверт {} > сегмента", env.len());
    let td = encode(&TunnelDataPayload { tunnel_id: vec![0; 16], seq: 0, data: env.clone(), hops_left: 6 }).unwrap();
    assert!(td.len() <= p2p_overlay::transport::framing::MAX_FRAME_PAYLOAD, "кадр {} > MAX_FRAME_PAYLOAD", td.len());
    // байтовые поля — bin: накладные расходы порядка сотен байт (NodeId — массив фиксированной длины, его размер
    // зависит от значений байтов), а не ≈ +50%, как при кодировании Vec<u8> массивом чисел
    assert!(env.len() < MAX_BLOCK + 1024, "байтовые поля — bin, без раздувания: {} байт", env.len());
}

#[test]
fn downloads_listed_by_sender_folder() {
    let d = tmp("folders");
    let data = sample(700);
    let mut st = FileStore::new(&d, 1 << 20, 60_000);
    let b = p2p_overlay::types::NodeId([0x42; 32]);
    for (k, from, name) in [(1u8, A, "a.txt"), (2, b, "b.txt"), (3, b, "c.txt")] {
        let mut o = offer(name, &data);
        o.transfer_id = vec![k; 16];
        st.on_offer(from, o, false).unwrap();
        let mut bl = block(&data, 0);
        bl.transfer_id = vec![k; 16];
        st.on_block(from, &bl).unwrap();
        st.finish(&[k; 16]).unwrap();
    }
    let v = p2p_overlay::app::files::list_downloads(&d.join("downloads"));
    let folders = v.as_array().unwrap();
    assert_eq!(folders.len(), 2, "по папке на отправителя: {v}");
    let fb = folders.iter().find(|f| f["folder"] == "4242424242424242").unwrap();
    assert_eq!(fb["files"].as_array().unwrap().len(), 2);
}

#[test]
fn real_extension_and_spoofed_executable_names() {
    use p2p_overlay::app::files::{file_ext, name_warning, visible_name};
    let spoof = "счёт_\u{202e}fdp.exe"; // на экране — «счёт_exe.pdf»
    assert_eq!(file_ext(spoof), "exe", "настоящее расширение — по концу имени");
    let w = name_warning(spoof).expect("исполняемый + скрытый символ — предупреждение");
    assert!(w.contains("U+202E") && w.contains(".exe"), "{w}");
    assert_eq!(visible_name(spoof), "счёт_⟨U+202E⟩fdp.exe");
    assert!(name_warning("setup.exe").is_none(), "просто исполняемый — без предупреждения (расширение показано отдельно)");
    assert!(name_warning("שלום\u{200f}.pdf").is_none(), "знак направления в имени документа — законен");
    assert!(name_warning("emoji\u{200d}.png").is_none());
    assert_eq!(file_ext("archive.tar.GZ"), "gz");
    assert_eq!(file_ext("README"), "");
}
