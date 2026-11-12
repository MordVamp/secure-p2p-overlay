//! Данные на диске: шифрование истории, смена ключа, срок хранения (удалить / перешифровать и оставить /
//! вынести в открытый файл), права доступа.

use p2p_overlay::app::history::{History, HistoryEntry};
use p2p_overlay::identity::node_identity::NodeIdentity;

const DAY: u64 = 24 * 3600 * 1000;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("p2p-hist-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn msg(ts: u64, peer: char, text: &str) -> HistoryEntry {
    let mut e = HistoryEntry::status_update(&peer.to_string().repeat(64), &format!("{ts:032x}"), Some("received"), None);
    e.dir = "in".into();
    e.ts_ms = ts;
    e.text = Some(text.into());
    e
}

fn now() -> u64 { p2p_overlay::types::now_ms() }

#[tokio::test]
async fn plaintext_history_is_encrypted_on_first_start_with_key() {
    let d = dir("migrate");
    let h = History::open(&d);
    h.append(&msg(now(), 'a', "секрет до шифрования")).await;
    drop(h);
    assert!(std::fs::read_to_string(d.join("history.jsonl")).unwrap().contains("секрет до шифрования"));
    let key = [7u8; 32];
    let h = History::open_with(&d, Some(&key), 365);
    h.append(&msg(now(), 'b', "секрет после")).await;
    let raw = std::fs::read_to_string(d.join("history.jsonl")).unwrap();
    assert!(raw.lines().all(|l| l.starts_with("enc1:")), "на диске только зашифрованные строки");
    assert!(!raw.contains("секрет"), "текста на диске нет");
    let t = h.tail(10).await;
    assert_eq!(t.len(), 2, "оба сообщения читаются");
    assert!(h.encrypted().await);
    // другой ключ — история не читается (и не ломается)
    drop(h);
    let other = History::open_with(&d, Some(&[9u8; 32]), 365);
    assert!(other.tail(10).await.is_empty() || !other.encrypted().await);
}

#[tokio::test]
async fn retention_delete_keep_and_export() {
    let d = dir("retention");
    let key = [3u8; 32];
    let h = History::open_with(&d, Some(&key), 365);
    let t = now();
    h.append(&msg(t - 400 * DAY, 'a', "старое 1")).await;
    h.append(&msg(t - 380 * DAY, 'b', "старое 2")).await;
    h.append(&msg(t - 10 * DAY, 'a', "свежее")).await;
    let r = h.retention(t).await;
    assert_eq!(r["expired"], 2);
    assert_eq!(r["due"], true, "{r}");

    // перешифровать и оставить: новый ключ, вопрос не повторяется год
    let k0 = r["key_id"].as_str().unwrap().to_owned();
    let v = h.reencrypt(t, true).await.unwrap();
    assert_ne!(v["key_id"].as_str().unwrap(), k0, "ключ сменился");
    let r = h.retention(t).await;
    assert_eq!(r["due"], false, "отложено: {r}");
    assert_eq!(h.tail(10).await.len(), 3, "всё читается новым ключом");
    let keys = std::fs::read_to_string(d.join("history.keys")).unwrap();
    assert!(!keys.contains(&k0), "старый ключ стёрт");

    // вынести в открытый файл: файл с предупреждением, записи убраны из истории
    let (path, n) = h.export_expired(t).await.unwrap();
    assert_eq!(n, 2);
    let txt = std::fs::read_to_string(&path).unwrap();
    assert!(txt.contains("НЕ ЗАШИФРОВАН") && txt.contains("старое 1") && txt.contains("старое 2"));
    assert_eq!(h.tail(10).await.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "открытый файл — только владельцу");
    }

    // удалить: свежее остаётся
    h.append(&msg(t - 500 * DAY, 'c', "очень старое")).await;
    assert_eq!(h.delete_expired(t).await.unwrap(), 1);
    assert_eq!(h.tail(10).await.len(), 1);
}

#[tokio::test]
async fn delete_peer_and_lookups_work_encrypted() {
    let d = dir("peer");
    let h = History::open_with(&d, Some(&[5u8; 32]), 365);
    let t = now();
    h.append(&msg(t, 'a', "a1")).await;
    h.append(&msg(t + 1, 'b', "b1")).await;
    assert!(h.has_incoming(&format!("{t:032x}")).await, "поиск по зашифрованной истории");
    assert!(!h.has_incoming("ffff").await);
    assert_eq!(h.peers().await.len(), 2);
    assert_eq!(h.delete_peer(&"a".repeat(64)).await.unwrap(), 1);
    assert_eq!(h.peers().await, vec!["b".repeat(64)]);
}

#[cfg(unix)]
#[test]
fn state_dir_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("perm");
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    let id = NodeIdentity::load_or_create_with_kek(&d, Some(&[1u8; 32])).unwrap();
    assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700, "каталог узла — только владельцу");
    assert!(id.data_key().is_some(), "с KEK есть ключ для данных");
    let no = NodeIdentity::load_or_create_with_kek(&dir("perm2"), None).unwrap();
    assert!(no.data_key().is_none());
}
