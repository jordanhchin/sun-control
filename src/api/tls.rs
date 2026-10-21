//! Certificate pinning for Sunshine's self-signed admin certificate.
//!
//! Sunshine generates its own certificate, so normal CA validation can never
//! succeed. Instead we trust on first use: the SHA-256 fingerprint of the
//! server's leaf certificate is stored in the host profile and every later
//! connection must present the same certificate.

use std::sync::{Arc, Mutex};

use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, ring};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};

/// Fingerprint formatted as uppercase colon-separated hex, like `openssl x509 -fingerprint`.
pub fn fingerprint(cert: &[u8]) -> String {
    let digest = Sha256::digest(cert);
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Trust {
    /// Only accept a certificate with this fingerprint.
    Pinned(String),
    /// Accept any certificate (first contact or `--insecure`).
    Any,
}

#[derive(Debug)]
pub struct PinningVerifier {
    trust: Trust,
    algorithms: WebPkiSupportedAlgorithms,
    /// Fingerprint of the last certificate the server presented.
    seen: Arc<Mutex<Option<String>>>,
}

impl PinningVerifier {
    pub fn new(trust: Trust, seen: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            trust,
            algorithms: ring::default_provider().signature_verification_algorithms,
            seen,
        }
    }
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity);
        *self.seen.lock().unwrap() = Some(fp.clone());
        match &self.trust {
            Trust::Any => Ok(ServerCertVerified::assertion()),
            Trust::Pinned(expected) if expected.eq_ignore_ascii_case(&fp) => {
                Ok(ServerCertVerified::assertion())
            }
            Trust::Pinned(_) => Err(rustls::Error::General(format!(
                "certificate fingerprint mismatch (server presented {fp})"
            ))),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

pub fn client_config(trust: Trust, seen: Arc<Mutex<Option<String>>>) -> rustls::ClientConfig {
    let provider = Arc::new(ring::default_provider());
    rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring supports default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinningVerifier::new(trust, seen)))
        .with_no_client_auth()
}
