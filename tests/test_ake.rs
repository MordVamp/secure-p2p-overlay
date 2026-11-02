//! Собственное рукопожатие (ТЗ §11.3) и обязательные отрицательные тесты §11.4:
//! подмена identity-ключа, несоответствие NodeID, изменение эфемерного ключа и других полей рукопожатия,
//! изменение шифртекста, повтор записи в текущей сессии и записи из завершённой сессии.

use std::sync::{Arc, Mutex};
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::security::ake::{AkeSecurity, Finish, Hello, Reply};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};

fn ident(tag: &str) -> NodeIdentity {
    let d = std::env::temp_dir().join(format!("p2p-ake-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    NodeIdentity::load_or_create(&d).unwrap()
}

type Hook = Arc<Mutex<dyn FnMut(u8, usize, Vec<u8>) -> Vec<Vec<u8>> + Send>>;

/// «Человек посередине»: пересылает сообщения с 4-байтовой длиной (и рукопожатие, и записи),
/// давая хуку изменить, удвоить или подменить сообщение. dir 0 — клиент→сервер, 1 — сервер→клиент.
fn mitm(hook: Hook) -> (DuplexStream, DuplexStream) {
    let (client, mid_a) = duplex(1 << 20);
    let (mid_b, server) = duplex(1 << 20);
    let (ar, aw) = tokio::io::split(mid_a);
    let (br, bw) = tokio::io::split(mid_b);
    for (dir, mut r, mut w) in [(0u8, Box::new(ar) as Box<dyn tokio::io::AsyncRead + Unpin + Send>, Box::new(bw) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>),
                                (1u8, Box::new(br), Box::new(aw))] {
        let hook = hook.clone();
        tokio::spawn(async move {
            let mut idx = 0;
            loop {
                let mut len = [0u8; 4];
                if r.read_exact(&mut len).await.is_err() { break; }
                let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
                if r.read_exact(&mut body).await.is_err() { break; }
                let out = (hook.lock().unwrap())(dir, idx, body);
                idx += 1;
                for m in out {
                    if w.write_all(&(m.len() as u32).to_be_bytes()).await.is_err() { return; }
                    if w.write_all(&m).await.is_err() { return; }
                }
                let _ = w.flush().await;
            }
            let _ = w.shutdown().await;   // обрыв с одной стороны → EOF на другой (иначе ответчик ждёт вечно)
        });
    }
    (client, server)
}

/// Тест не должен зависать: рукопожатие обязано завершиться (успехом или отказом) за 5 с.
async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(5), f).await.expect("рукопожатие зависло")
}

/// Инициатор отправляет Finish вместе с первой записью; в тестах без данных — явный flush.
async fn connect_flushed(sa: &AkeSecurity, c: DuplexStream, exp: Option<p2p_overlay::types::NodeId>)
    -> anyhow::Result<(p2p_overlay::security::ake::AkeStream<DuplexStream>, p2p_overlay::types::NodeId)> {
    let (mut st, id) = sa.connect(c, exp.as_ref()).await?;
    st.flush().await?;
    Ok((st, id))
}

fn pass() -> Hook { Arc::new(Mutex::new(|_: u8, _: usize, b: Vec<u8>| vec![b])) }

fn enc<T: serde::Serialize>(m: &T) -> Vec<u8> { rmp_serde::to_vec_named(m).unwrap() }

#[tokio::test]
async fn handshake_authenticates_both_sides_and_carries_data() {
    let (a, b) = (ident("ok-a"), ident("ok-b"));
    let (sa, sb) = (AkeSecurity::from_identity(&a), AkeSecurity::from_identity(&b));
    let (c, s) = mitm(pass());
    let bid = b.node_id;
    let (cl, sv) = within(async { tokio::join!(connect_flushed(&sa, c, Some(bid)), sb.accept(s)) }).await;
    let ((mut cl, peer_b), (mut sv, peer_a)) = (cl.unwrap(), sv.unwrap());
    assert_eq!((peer_b, peer_a), (b.node_id, a.node_id));
    assert_eq!(cl.session_id(), sv.session_id());
    // больше одной записи (16 КиБ) в обе стороны
    let data: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    cl.write_all(&data).await.unwrap(); cl.flush().await.unwrap();
    let mut got = vec![0u8; data.len()];
    sv.read_exact(&mut got).await.unwrap();
    assert_eq!(got, data);
    sv.write_all(b"pong").await.unwrap(); sv.flush().await.unwrap();
    let mut p = [0u8; 4];
    cl.read_exact(&mut p).await.unwrap();
    assert_eq!(&p, b"pong");
}

#[tokio::test]
async fn new_session_id_and_keys_on_every_connection() {
    let (a, b) = (ident("re-a"), ident("re-b"));
    let (sa, sb) = (AkeSecurity::from_identity(&a), AkeSecurity::from_identity(&b));
    let mut sids = vec![];
    for _ in 0..2 {
        let (c, s) = mitm(pass());
        let (cl, sv) = within(async { tokio::join!(connect_flushed(&sa, c, None), sb.accept(s)) }).await;
        let ((cl, _), _) = (cl.unwrap(), sv.unwrap());
        sids.push(cl.session_id());
    }
    assert_ne!(sids[0], sids[1], "после переподключения — новая session_id");
}

#[tokio::test]
async fn unexpected_node_id_rejected() {
    let (a, b, other) = (ident("exp-a"), ident("exp-b"), ident("exp-o"));
    let (sa, sb) = (AkeSecurity::from_identity(&a), AkeSecurity::from_identity(&b));
    let (c, s) = mitm(pass());
    let oid = other.node_id;
    let (cl, _sv) = within(async { tokio::join!(connect_flushed(&sa, c, Some(oid)), sb.accept(s)) }).await;
    assert!(cl.is_err(), "ответчик не тот, к кому подключались");
}

#[tokio::test]
async fn node_id_not_matching_identity_key_rejected() {
    // Ева присылает NodeID жертвы со своим ключом
    let (b, victim, eve) = (ident("nid-b"), ident("nid-v"), ident("nid-e"));
    let sb = AkeSecurity::from_identity(&b);
    let (mut c, s) = duplex(1 << 16);
    let h = Hello { version: 1, node_id: victim.node_id.0.to_vec(), identity_pk: eve.public_key_bytes().to_vec(),
                    eph_pk: vec![9; 32], nonce: vec![1; 32] };
    let m = enc(&h);
    c.write_all(&(m.len() as u32).to_be_bytes()).await.unwrap();
    c.write_all(&m).await.unwrap();
    assert!(sb.accept(s).await.is_err());
}

#[tokio::test]
async fn substituted_identity_key_without_private_key_rejected() {
    // Ева выдаёт себя за жертву: правильная пара NodeID/ключ жертвы, но подписать транскрипт она может только своим ключом
    let (b, victim, eve) = (ident("sub-b"), ident("sub-v"), ident("sub-e"));
    let sb = AkeSecurity::from_identity(&b);
    let (mut c, s) = duplex(1 << 16);
    let server = tokio::spawn(async move { sb.accept(s).await.map(|_| ()) });
    let eph = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::random_from_rng(rand::rngs::OsRng));
    let h = Hello { version: 1, node_id: victim.node_id.0.to_vec(), identity_pk: victim.public_key_bytes().to_vec(),
                    eph_pk: eph.to_bytes().to_vec(), nonce: vec![7; 32] };
    let m = enc(&h);
    c.write_all(&(m.len() as u32).to_be_bytes()).await.unwrap();
    c.write_all(&m).await.unwrap();
    let mut len = [0u8; 4];
    c.read_exact(&mut len).await.unwrap();
    let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
    c.read_exact(&mut body).await.unwrap();
    let r: Reply = rmp_serde::from_slice(&body).unwrap();
    let t = p2p_overlay::security::ake::transcript(&h, &r.node_id, &r.identity_pk, &r.eph_pk, &r.nonce, &r.session_id);
    let mut msg = b"p2p-ake-v1 initiator signature".to_vec();
    msg.extend_from_slice(&t);
    let f = enc(&Finish { signature: eve.sign(&msg).to_bytes().to_vec() });
    c.write_all(&(f.len() as u32).to_be_bytes()).await.unwrap();
    c.write_all(&f).await.unwrap();
    assert!(server.await.unwrap().is_err(), "подпись не ключом жертвы — отказ");
}

#[tokio::test]
async fn modified_ephemeral_key_rejected() {
    let (a, b) = (ident("eph-a"), ident("eph-b"));
    let (sa, sb) = (AkeSecurity::from_identity(&a), AkeSecurity::from_identity(&b));
    // MITM подменяет эфемерный ключ инициатора своим (классическая атака на неаутентифицированный DH)
    let hook: Hook = Arc::new(Mutex::new(|dir: u8, idx: usize, body: Vec<u8>| {
        if dir == 0 && idx == 0 {
            let mut h: Hello = rmp_serde::from_slice(&body).unwrap();
            let mitm_eph = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::random_from_rng(rand::rngs::OsRng));
            h.eph_pk = mitm_eph.to_bytes().to_vec();
            return vec![enc(&h)];
        }
        vec![body]
    }));
    let (c, s) = mitm(hook);
    let (cl, sv) = within(async { tokio::join!(connect_flushed(&sa, c, None), sb.accept(s)) }).await;
    assert!(cl.is_err(), "подпись ответчика покрывает подменённый ключ — инициатор её не примет");
    assert!(sv.is_err());
}

#[tokio::test]
async fn modified_handshake_field_rejected() {
    for field in ["nonce", "session_id"] {
        let (a, b) = (ident(&format!("fld-a-{field}")), ident(&format!("fld-b-{field}")));
        let (sa, sb) = (AkeSecurity::from_identity(&a), AkeSecurity::from_identity(&b));
        let f = field.to_owned();
        let hook: Hook = Arc::new(Mutex::new(move |dir: u8, idx: usize, body: Vec<u8>| {
            if dir == 1 && idx == 0 {
                let mut r: Reply = rmp_serde::from_slice(&body).unwrap();
                if f == "nonce" { r.nonce[0] ^= 1 } else { r.session_id[0] ^= 1 }
                return vec![enc(&r)];
            }
            vec![body]
        }));
        let (c, s) = mitm(hook);
        let (cl, _sv) = within(async { tokio::join!(connect_flushed(&sa, c, None), sb.accept(s)) }).await;
        assert!(cl.is_err(), "изменённое поле {field} в Reply");
    }
}

/// Рукопожатие через MITM с хуком на записи после него (сообщения клиента с номера 2: Hello=0, Finish=1).
async fn session_with(a: &NodeIdentity, b: &NodeIdentity, hook: Hook) -> (p2p_overlay::security::ake::AkeStream<DuplexStream>, p2p_overlay::security::ake::AkeStream<DuplexStream>) {
    let (sa, sb) = (AkeSecurity::from_identity(a), AkeSecurity::from_identity(b));
    let (c, s) = mitm(hook);
    let (cl, sv) = within(async { tokio::join!(connect_flushed(&sa, c, None), sb.accept(s)) }).await;
    (cl.unwrap().0, sv.unwrap().0)
}

#[tokio::test]
async fn modified_ciphertext_rejected() {
    let hook: Hook = Arc::new(Mutex::new(|dir: u8, idx: usize, mut body: Vec<u8>| {
        if dir == 0 && idx == 2 { let n = body.len(); body[n - 1] ^= 0x80; }
        vec![body]
    }));
    let (mut cl, mut sv) = session_with(&ident("ct-a"), &ident("ct-b"), hook).await;
    cl.write_all(b"secret").await.unwrap(); cl.flush().await.unwrap();
    let mut buf = [0u8; 6];
    assert!(sv.read_exact(&mut buf).await.is_err(), "изменённый шифртекст не расшифровывается");
}

#[tokio::test]
async fn replayed_record_in_current_session_rejected() {
    let hook: Hook = Arc::new(Mutex::new(|dir: u8, idx: usize, body: Vec<u8>| {
        if dir == 0 && idx == 2 { vec![body.clone(), body] } else { vec![body] }   // повтор первой записи
    }));
    let (mut cl, mut sv) = session_with(&ident("rp-a"), &ident("rp-b"), hook).await;
    cl.write_all(b"pay-1").await.unwrap(); cl.flush().await.unwrap();
    let mut buf = [0u8; 5];
    sv.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pay-1");
    let err = sv.read_exact(&mut buf).await.unwrap_err();
    assert!(err.to_string().contains("повтор"), "{err}");
}

#[tokio::test]
async fn record_from_finished_session_rejected() {
    // сессия 1: перехватываем первую запись клиента
    let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let cap = captured.clone();
    let hook1: Hook = Arc::new(Mutex::new(move |dir: u8, idx: usize, body: Vec<u8>| {
        if dir == 0 && idx == 2 { *cap.lock().unwrap() = Some(body.clone()); }
        vec![body]
    }));
    let (a, b) = (ident("old-a"), ident("old-b"));
    let (mut cl, mut sv) = session_with(&a, &b, hook1).await;
    cl.write_all(b"transfer 100").await.unwrap(); cl.flush().await.unwrap();
    let mut buf = [0u8; 12];
    sv.read_exact(&mut buf).await.unwrap();
    drop((cl, sv));
    let old = captured.lock().unwrap().clone().unwrap();
    // сессия 2 тех же узлов: вместо первой записи клиента MITM подставляет запись из сессии 1 (счётчик тот же — 0)
    let hook2: Hook = Arc::new(Mutex::new(move |dir: u8, idx: usize, body: Vec<u8>| {
        if dir == 0 && idx == 2 { vec![old.clone()] } else { vec![body] }
    }));
    let (mut cl, mut sv) = session_with(&a, &b, hook2).await;
    cl.write_all(b"transfer 999").await.unwrap(); cl.flush().await.unwrap();
    let err = sv.read_exact(&mut buf).await.unwrap_err();
    assert!(err.to_string().contains("подлинности"), "{err}");
}
