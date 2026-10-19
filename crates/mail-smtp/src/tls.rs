//! TLS for SMTP. Kept in step with `mail-imap/src/net.rs`: the two crates may not depend on
//! each other (section 3), so the verifier is repeated here.

use crate::trace::{Direction, Trace};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use lettre::transport::smtp::client::AsyncTokioStream;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use crate::SmtpError;

type Seen = Arc<Mutex<Option<[u8; 32]>>>;

pub(crate) async fn handshake(
    tcp: TcpStream,
    host: &str,
    pin: Option<[u8; 32]>,
    timeout: Duration,
) -> Result<TlsStream<TcpStream>, SmtpError> {
    let seen: Seen = Arc::new(Mutex::new(None));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let platform = rustls_platform_verifier::Verifier::new(Arc::clone(&provider))
        .map_err(|e| SmtpError::Tls(e.to_string()))?;
    let verifier = PinningVerifier {
        platform,
        provider: Arc::clone(&provider),
        pin,
        seen: Arc::clone(&seen),
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| SmtpError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| SmtpError::Tls("invalid server name".into()))?;
    let connect = TlsConnector::from(Arc::new(config)).connect(name, tcp);
    match tokio::time::timeout(timeout, connect).await {
        Err(_) => Err(SmtpError::Timeout),
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(err)) => Err(handshake_error(&err, &seen)),
    }
}

fn handshake_error(err: &io::Error, seen: &Seen) -> SmtpError {
    let tls = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    match tls {
        Some(rustls::Error::InvalidCertificate(reason)) => {
            let sha256 = seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .map(|h| h.iter().map(|b| format!("{b:02x}")).collect())
                .unwrap_or_default();
            let reason = match reason {
                CertificateError::ApplicationVerificationFailure => {
                    "certificate does not match the pinned fingerprint".to_string()
                }
                other => format!("{other:?}"),
            };
            SmtpError::Certificate { reason, sha256 }
        }
        Some(other) => SmtpError::Tls(other.to_string()),
        None => SmtpError::Tls(err.kind().to_string()),
    }
}

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

/// The encrypted stream handed to lettre, with optional bytes read before the stream's own.
#[derive(Debug)]
pub(crate) struct Prefixed {
    prefix: Vec<u8>,
    offset: usize,
    inner: TlsStream<TcpStream>,
    /// The debug trace, when it was on at connect time (section 13, item 8).
    trace: Option<Trace>,
}

impl Prefixed {
    pub(crate) fn new(inner: TlsStream<TcpStream>, prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            offset: 0,
            inner,
            trace: Trace::new_if_enabled(),
        }
    }
}

impl AsyncRead for Prefixed {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.offset < this.prefix.len() {
            let rest = &this.prefix[this.offset..];
            let n = rest.len().min(buf.remaining());
            buf.put_slice(&rest[..n]);
            this.offset += n;
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        let poll = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let (Poll::Ready(Ok(())), Some(trace)) = (&poll, this.trace.as_mut()) {
            trace.log(Direction::Server, &buf.filled()[before..]);
        }
        poll
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let (Poll::Ready(Ok(n)), Some(trace)) = (&poll, this.trace.as_mut()) {
            trace.log(Direction::Client, &buf[..*n]);
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl AsyncTokioStream for Prefixed {
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().0.peer_addr()
    }
}
