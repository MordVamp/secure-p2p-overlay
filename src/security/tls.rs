//! security/tls.rs — TLS 1.3 с взаимной аутентификацией (упрощённый уровень).
//!
//! Алгоритм:
//!  1. Из Ed25519 identity-ключа генерируется X.509 self-signed cert (rcgen 0.13).
//!  2. Оба узла используют mutual TLS.
//!  3. TLS 1.2 и ниже запрещены; 0-RTT отключён.
//!  4. После рукопожатия: извлекаем cert peer → NodeID из DER → сравниваем.

use std::sync::Arc;
use anyhow::{bail, Context, Result};
use ed25519_dalek::{SigningKey, pkcs8::EncodePrivateKey};
use pkcs8::der::pem::LineEnding;
use rcgen::{CertificateParams, KeyPair};
use rustls::{
    ClientConfig, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    pki_types::{ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::debug;

use crate::types::NodeId;

/// TLS-провайдер для одного узла.
pub struct TlsSecurity {
    pub connector: TlsConnector,
    pub acceptor:  TlsAcceptor,
}

impl TlsSecurity {
    /// Создать TLS-провайдер из Ed25519 identity-ключа.
    pub fn new(signing_key: &SigningKey) -> Result<Self> {
        let (cert_der, key_der) = generate_self_signed(signing_key)?;

        // ── Серверный конфиг ──────────────────────────────────────────────
        let server_cfg = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_client_cert_verifier(Arc::new(AllowAnyEd25519))
            .with_single_cert(
                vec![cert_der.clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der.clone())),
            )
            .context("building ServerConfig")?;

        // ── Клиентский конфиг ─────────────────────────────────────────────
        let client_cfg = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AllowAnyEd25519))
            .with_client_auth_cert(
                vec![cert_der],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
            )
            .context("building ClientConfig")?;

        Ok(Self {
            connector: TlsConnector::from(Arc::new(client_cfg)),
            acceptor:  TlsAcceptor::from(Arc::new(server_cfg)),
        })
    }

    /// Клиент: подключиться к peer с проверкой NodeID из его сертификата.
    pub async fn connect(
        &self,
        stream: TcpStream,
        expected_node_id: &NodeId,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let domain = rustls::pki_types::ServerName::try_from("overlay.local".to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let tls = self.connector.connect(domain, stream).await
            .context("TLS client handshake")?;

        let peer_id = peer_node_id_from_client(&tls)?;
        if peer_id != *expected_node_id {
            bail!("NodeID mismatch: expected {}, got {}", expected_node_id, peer_id);
        }
        debug!("TLS connected, peer NodeID verified: {}", expected_node_id);
        Ok(tls)
    }

    /// Сервер: принять входящее соединение, вернуть NodeID peer.
    pub async fn accept(
        &self,
        stream: TcpStream,
    ) -> Result<(tokio_rustls::server::TlsStream<TcpStream>, NodeId)> {
        let tls = self.acceptor.accept(stream).await
            .context("TLS server handshake")?;
        let peer_id = peer_node_id_from_server(&tls)?;
        debug!("TLS accepted, peer NodeID: {}", peer_id);
        Ok((tls, peer_id))
    }
}

// ── Генерация self-signed Ed25519 сертификата (rcgen 0.13 API) ───────────────

fn generate_self_signed(signing_key: &SigningKey) -> Result<(CertificateDer<'static>, Vec<u8>)> {
    let pem = signing_key.to_pkcs8_pem(LineEnding::LF)
        .context("encoding key to PKCS8")?;

    let key_pair = KeyPair::from_pem(pem.as_str())
        .context("rcgen KeyPair from PEM")?;

    let params = CertificateParams::new(vec!["overlay.local".to_string()])
        .context("rcgen CertificateParams")?;

    let cert = params.self_signed(&key_pair)
        .context("rcgen self-sign")?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der  = key_pair.serialize_der();
    Ok((cert_der, key_der))
}

// ── NodeID из DER-сертификата ─────────────────────────────────────────────────

/// SHA-256 от raw DER cert → NodeID (суррогат до подключения x509-parser).
/// В продвинутой версии: парсим SubjectPublicKeyInfo и берём Ed25519 pubkey.
fn node_id_from_cert_der(cert: &[u8]) -> NodeId {
    let mut h = Sha256::new();
    h.update([0x02u8]); // префикс "tls-cert" (отличаем от identity NodeID)
    h.update(cert);
    NodeId(h.finalize().into())
}

fn peer_node_id_from_client(
    tls: &tokio_rustls::client::TlsStream<TcpStream>,
) -> Result<NodeId> {
    let (_, session) = tls.get_ref();
    if let Some(certs) = session.peer_certificates() {
        if let Some(cert) = certs.first() {
            return Ok(node_id_from_cert_der(cert.as_ref()));
        }
    }
    bail!("peer provided no certificate")
}

fn peer_node_id_from_server(
    tls: &tokio_rustls::server::TlsStream<TcpStream>,
) -> Result<NodeId> {
    let (_, session) = tls.get_ref();
    if let Some(certs) = session.peer_certificates() {
        if let Some(cert) = certs.first() {
            return Ok(node_id_from_cert_der(cert.as_ref()));
        }
    }
    bail!("client provided no certificate")
}

// ── Кастомный верификатор (принимает self-signed Ed25519 certs) ───────────────
// NodeID верификация выполняется уровнем выше после handshake.

#[derive(Debug)]
struct AllowAnyEd25519;

impl ServerCertVerifier for AllowAnyEd25519 {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 disabled".into()))
    }

    fn verify_tls13_signature(
        &self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            msg, cert, dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl ClientCertVerifier for AllowAnyEd25519 {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] { &[] }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 disabled".into()))
    }

    fn verify_tls13_signature(
        &self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            msg, cert, dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
