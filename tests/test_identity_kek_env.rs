//! KEK через окружение (`P2P_IDENTITY_KEK`, так его передаёт Android-приложение): читается и сразу удаляется.
//! Отдельный файл — единственный тест процесса: окружение общее для всех потоков.

use p2p_overlay::identity::node_identity::NodeIdentity;

#[test]
fn kek_from_environment_is_used_and_removed() {
    let d = std::env::temp_dir().join(format!("p2p-kek-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let plain = NodeIdentity::load_or_create(&d).unwrap();
    std::env::set_var("P2P_IDENTITY_KEK", "33".repeat(32));
    let enc = NodeIdentity::load_or_create(&d).unwrap();
    assert_eq!(enc.node_id, plain.node_id);
    assert!(d.join("identity.key.enc").exists() && !d.join("identity.key").exists());
    assert!(std::env::var("P2P_IDENTITY_KEK").is_err(), "KEK не остаётся в окружении процесса");
    assert!(NodeIdentity::load_or_create(&d).is_err(), "без KEK зашифрованный ключ не открыть");
    std::env::set_var("P2P_IDENTITY_KEK", "zz");
    assert!(NodeIdentity::load_or_create(&d).is_err(), "KEK не hex");
}
