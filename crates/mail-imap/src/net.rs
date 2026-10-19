//! TCP and TLS for IMAP connections (sections 5 and 12).
//!
//! TLS 1.2 or newer through rustls on the `ring` provider, certificates verified by the OS
//! trust store (SPEC_GAPS G30). A per-account pin replaces chain verification with an exact
//! match on the leaf certificate's SHA-256, which is how self-signed servers work; a pinned
//! certificate that changes is a hard failure. `mail-smtp` keeps a copy of this file's TLS
//! part, because the two crates may not depend on each other (section 3).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use crate::{ImapError, TCP_KEEPALIVE};

/// Opens a TCP connection with the section 12 keepalive, within `timeout`.
pub(crate) async fn tcp_connect(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<TcpStream, ImapError> {
    let stream = tokio::time::timeout(timeout, TcpStream::connect((host, port)))
        .await
        .map_err(|_| ImapError::Timeout)?
        .map_err(|e| ImapError::Network(e.kind().to_string()))?;
    let keepalive = TcpKeepalive::new()
        .with_time(TCP_KEEPALIVE)
        .with_interval(TCP_KEEPALIVE);
    SockRef::from(&stream)
        .set_tcp_keepalive(&keepalive)
        .map_err(|e| ImapError::Network(e.kind().to_string()))?;
    stream
        .set_nodelay(true)
        .map_err(|e| ImapError::Network(e.kind().to_string()))?;
    Ok(stream)
}

/// Runs the TLS handshake over `tcp`. A certificate failure reports the presented leaf
/// certificate's SHA-256 so the user can pin it (section 5, Certificates).
pub(crate) async fn tls_handshake(
    tcp: TcpStream,
    host: &str,
    pin: Option<[u8; 32]>,
    timeout: Duration,
) -> Result<TlsStream<TcpStream>, ImapError> {
    let seen = Arc::new(Mutex::new(None));
    let config = client_config(pin, Arc::clone(&seen))?;
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| ImapError::Tls("invalid server name".into()))?;
    let handshake = TlsConnector::from(Arc::new(config)).connect(name, tcp);
    match tokio::time::timeout(timeout, handshake).await {
        Err(_) => Err(ImapError::Timeout),
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(err)) => Err(handshake_error(&err, &seen)),
    }
}

/// Lowercase hex of a SHA-256 digest.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses a 64-character hex fingerprint, as stored in `accounts.pinned_cert_sha256`.
pub fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

type Seen = Arc<Mutex<Option<[u8; 32]>>>;

fn client_config(pin: Option<[u8; 32]>, seen: Seen) -> Result<ClientConfig, ImapError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let platform = rustls_platform_verifier::Verifier::new(Arc::clone(&provider))
        .map_err(|e| ImapError::Tls(e.to_string()))?;
    let verifier = PinningVerifier {
        platform,
        provider: Arc::clone(&provider),
        pin,
        seen,
    };
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ImapError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

fn handshake_error(err: &std::io::Error, seen: &Seen) -> ImapError {
    let tls = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    match tls {
        Some(rustls::Error::InvalidCertificate(reason)) => {
            let sha256 = seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .map(|h| hex(&h))
                .unwrap_or_default();
            let reason = match reason {
                CertificateError::ApplicationVerificationFailure => {
                    "certificate does not match the pinned fingerprint".to_string()
                }
                other => format!("{other:?}"),
            };
            ImapError::Certificate { reason, sha256 }
        }
        Some(other) => ImapError::Tls(other.to_string()),
        None => ImapError::Network(err.kind().to_string()),
    }
}

/// The OS verifier, or an exact leaf match when the account has a pin.
#[derive(Debug)]
struct PinningVerifier {
    platform: rustls_platform_verifier::Verifier,
    provider: Arc<CryptoProvider>,
    pin: Option<[u8; 32]>,
    seen: Seen,
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(digest);
        match self.pin {
            Some(pin) if pin == digest => Ok(ServerCertVerified::assertion()),
            Some(_) => Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
            None => self.platform.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_round_trip() {
        let bytes: [u8; 32] = std::array::from_fn(|i| (i * 7) as u8);
        let text = hex(&bytes);
        assert_eq!(text.len(), 64);
        assert_eq!(parse_fingerprint(&text), Some(bytes));
        assert_eq!(parse_fingerprint(&text.to_uppercase()), Some(bytes));
        assert_eq!(parse_fingerprint("abc"), None);
        assert_eq!(parse_fingerprint(&"zz".repeat(32)), None);
    }

    #[tokio::test]
    async fn unreachable_port_is_a_network_error() {
        // Port 1 on loopback refuses connections on every CI OS.
        let err = tcp_connect("127.0.0.1", 1, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(matches!(err, ImapError::Network(_)), "{err:?}");
    }
}
