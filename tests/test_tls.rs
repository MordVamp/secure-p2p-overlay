//! Негативные и позитивные тесты TLS 1.3 (mutual, Ed25519, NodeID из сертификата).
//! Реальные сокеты на 127.0.0.1.

use std::sync::Arc;
use ed25519_dalek::SigningKey;
use p2p_overlay::identity::node_identity::compute_node_id;
use p2p_overlay::security::tls::TlsSecurity;
use p2p_overlay::types::NodeId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn key(b: u8) -> SigningKey { SigningKey::from_bytes(&[b; 32]) }
fn id_of(k: &SigningKey) -> NodeId { compute_node_id(&k.verifying_key()) }

/// Поднять TLS-сервер; он принимает одно соединение и возвращает результат accept.
async fn server(k: &SigningKey) -> (std::net::SocketAddr, tokio::task::JoinHandle<anyhow::Result<NodeId>>) {
    let tls = Arc::new(TlsSecurity::new(k).unwrap());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let h = tokio::spawn(async move {
        let (s, _) = l.accept().await?;
        let (mut stream, id) = tls.accept(s).await?;
        let mut b = [0u8; 4];
        stream.read_exact(&mut b).await?;
        stream.write_all(&b).await?;
        Ok(id)
    });
    (addr, h)
}

#[tokio::test]
async fn tls_mutual_auth_yields_real_node_ids() {
    let (ks, kc) = (key(1), key(2));
    let (addr, srv) = server(&ks).await;
    let client = TlsSecurity::new(&kc).unwrap();
    let (mut s, peer) = client.connect(TcpStream::connect(addr).await.unwrap(), Some(&id_of(&ks))).await.unwrap();
    assert_eq!(peer, id_of(&ks), "NodeID сервера из сертификата = identity NodeID");
    s.write_all(b"ping").await.unwrap();
    let mut b = [0u8; 4];
    s.read_exact(&mut b).await.unwrap();
    assert_eq!(&b, b"ping");
    assert_eq!(srv.await.unwrap().unwrap(), id_of(&kc), "сервер видит NodeID клиента");
}

#[tokio::test]
async fn tls_wrong_expected_node_id_rejected() {
    let (ks, kc) = (key(3), key(4));
    let (addr, _srv) = server(&ks).await;
    let client = TlsSecurity::new(&kc).unwrap();
    let wrong = id_of(&key(99));
    let r = client.connect(TcpStream::connect(addr).await.unwrap(), Some(&wrong)).await;
    assert!(r.is_err(), "подмена NodeID сервера должна отвергаться");
    assert!(r.err().unwrap().to_string().contains("NodeID mismatch"));
}

#[tokio::test]
async fn tls_plaintext_client_rejected() {
    let (addr, srv) = server(&key(5)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"\x00\x00\x00\x10 not a TLS hello at all").await.unwrap();
    assert!(srv.await.unwrap().is_err(), "не-TLS клиент должен отвергаться");
}

#[tokio::test]
async fn tls_garbage_bytes_rejected() {
    let (addr, srv) = server(&key(6)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(&[0x16, 0x03, 0x03, 0x00, 0x05, 1, 2, 3, 4, 5]).await.unwrap();
    assert!(srv.await.unwrap().is_err());
}

#[tokio::test]
async fn tls_client_without_certificate_rejected() {
    // Клиент без сертификата (обычный rustls без client auth) — mutual TLS обязан отказать
    let (addr, srv) = server(&key(7)).await;
    let roots = rustls::RootCertStore::empty();
    let cfg = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(roots).with_no_client_auth();
    let c = tokio_rustls::TlsConnector::from(Arc::new(cfg));
    let dom = rustls::pki_types::ServerName::try_from("overlay.local".to_string()).unwrap();
    let _ = c.connect(dom, TcpStream::connect(addr).await.unwrap()).await; // сам клиент не доверяет серверу
    assert!(srv.await.unwrap().is_err());
}

#[tokio::test]
async fn tls12_only_client_rejected() {
    let (addr, srv) = server(&key(8)).await;
    let cfg = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
        .with_root_certificates(rustls::RootCertStore::empty()).with_no_client_auth();
    let c = tokio_rustls::TlsConnector::from(Arc::new(cfg));
    let dom = rustls::pki_types::ServerName::try_from("overlay.local".to_string()).unwrap();
    let r = c.connect(dom, TcpStream::connect(addr).await.unwrap()).await;
    assert!(r.is_err(), "TLS 1.2 запрещён");
    assert!(srv.await.unwrap().is_err());
}

#[test]
fn node_id_from_cert_rejects_garbage() {
    assert!(p2p_overlay::security::tls::node_id_from_cert_der(&[1, 2, 3, 4]).is_err());
}
