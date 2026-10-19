#![forbid(unsafe_code)]
//! `smtp` (BUILD.md section 3): submission and outgoing MIME build. Never touches the
//! database.
//!
//! M2 builds the connection test of account setup (section 5, step 4; SPEC_GAPS G32):
//! connect, `EHLO`, `AUTH`, `QUIT`, with no message sent. Submission arrives in M7.
//!
//! TLS is ours, not lettre's, so the same OS verifier and per-account pin apply as for IMAP:
//! implicit TLS on 465, or required STARTTLS on 587 that aborts when STARTTLS is not offered
//! (section 5, Transport security). lettre then speaks SMTP over the encrypted stream.

mod error;
mod tls;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

pub use error::{ErrorClass, SmtpError};

use std::future::Future;
use std::net::Ipv4Addr;
use std::time::Duration;

use lettre::transport::smtp::authentication::{Credentials as LettreCredentials, Mechanism};
use lettre::transport::smtp::client::AsyncSmtpConnection;
use lettre::transport::smtp::extension::ClientId;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Timeout for each SMTP step (section 12: 60 s for normal commands).
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// Transport security. There is no plaintext option (section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Implicit TLS (465).
    Tls,
    /// Required STARTTLS (587).
    StartTls,
}

/// Where and how to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// SHA-256 of the pinned leaf certificate (section 5, Certificates).
    pub pinned_cert_sha256: Option<[u8; 32]>,
    pub connect_timeout: Duration,
}

/// Runs the SMTP half of Test connection. Real servers use [`SmtpClient`]; tests use
/// [`fake::FakeSmtp`].
pub trait SmtpCheck: Send + Sync {
    /// Connects, authenticates with `AUTH PLAIN` (or `LOGIN`) and quits. Sends no message.
    fn check_login(
        &self,
        config: &SmtpConfig,
        username: &str,
        password: &str,
    ) -> impl Future<Output = Result<(), SmtpError>> + Send;
}

/// Talks to real SMTP servers.
#[derive(Debug, Clone, Copy, Default)]
pub struct SmtpClient;

/// The EHLO name. An address literal, so the machine's host name never leaves it.
fn client_id() -> ClientId {
    ClientId::Ipv4(Ipv4Addr::LOCALHOST)
}

impl SmtpCheck for SmtpClient {
    async fn check_login(
        &self,
        config: &SmtpConfig,
        username: &str,
        password: &str,
    ) -> Result<(), SmtpError> {
        let timeout = config.connect_timeout;
        let tcp = tokio::time::timeout(
            timeout,
            TcpStream::connect((config.host.as_str(), config.port)),
        )
        .await
        .map_err(|_| SmtpError::Timeout)?
        .map_err(|e| SmtpError::Network(e.kind().to_string()))?;
        let mut conn = match config.security {
            Security::Tls => {
                let tls =
                    tls::handshake(tcp, &config.host, config.pinned_cert_sha256, timeout).await?;
                let stream = tls::Prefixed::new(tls, Vec::new());
                step(AsyncSmtpConnection::connect_with_transport(
                    Box::new(stream),
                    &client_id(),
                ))
                .await?
            }
            Security::StartTls => {
                let tcp = tokio::time::timeout(timeout, starttls(tcp))
                    .await
                    .map_err(|_| SmtpError::Timeout)??;
                let tls =
                    tls::handshake(tcp, &config.host, config.pinned_cert_sha256, timeout).await?;
                // No greeting follows STARTTLS; lettre expects one before its EHLO, so a
                // synthetic one is put in front of the encrypted stream.
                let stream = tls::Prefixed::new(tls, b"220 ready\r\n".to_vec());
                step(AsyncSmtpConnection::connect_with_transport(
                    Box::new(stream),
                    &client_id(),
                ))
                .await?
            }
        };
        let credentials = LettreCredentials::new(username.to_string(), password.to_string());
        let auth = step(conn.auth(&[Mechanism::Plain, Mechanism::Login], &credentials)).await;
        if let Err(err) = auth {
            conn.abort().await;
            return Err(match err {
                SmtpError::Rejected(_) => SmtpError::AuthFailed,
                other => other,
            });
        }
        // The login is what was tested; a failed QUIT changes nothing.
        let _ = step(conn.quit()).await;
        Ok(())
    }
}

/// One lettre step under the command timeout.
async fn step<T>(
    fut: impl Future<Output = Result<T, lettre::transport::smtp::Error>>,
) -> Result<T, SmtpError> {
    tokio::time::timeout(COMMAND_TIMEOUT, fut)
        .await
        .map_err(|_| SmtpError::Timeout)?
        .map_err(error::from_lettre)
}

/// The plaintext part of STARTTLS: greeting, `EHLO`, `STARTTLS`. Aborts when the server
/// does not offer STARTTLS; nothing else is ever sent in plaintext (section 5).
async fn starttls(tcp: TcpStream) -> Result<TcpStream, SmtpError> {
    let mut stream = BufReader::new(tcp);
    let (code, _) = reply(&mut stream).await?;
    if code != 220 {
        return Err(SmtpError::Rejected(format!("greeting {code}")));
    }
    let ehlo = format!("EHLO {}\r\n", client_id());
    stream
        .get_mut()
        .write_all(ehlo.as_bytes())
        .await
        .map_err(io_error)?;
    let (code, lines) = reply(&mut stream).await?;
    if code != 250 {
        return Err(SmtpError::Rejected(format!("EHLO {code}")));
    }
    let offered = lines
        .iter()
        .any(|l| l.trim().eq_ignore_ascii_case("STARTTLS"));
    if !offered {
        return Err(SmtpError::StartTlsUnavailable);
    }
    stream
        .get_mut()
        .write_all(b"STARTTLS\r\n")
        .await
        .map_err(io_error)?;
    let (code, _) = reply(&mut stream).await?;
    if code != 220 {
        return Err(SmtpError::StartTlsUnavailable);
    }
    if !stream.buffer().is_empty() {
        // Data sent before the handshake would be injected plaintext (CVE-2011-0411 class).
        return Err(SmtpError::Protocol("data after STARTTLS reply".into()));
    }
    Ok(stream.into_inner())
}

/// Reads one possibly multi-line reply: the code and the text of each line.
async fn reply(stream: &mut BufReader<TcpStream>) -> Result<(u16, Vec<String>), SmtpError> {
    const MAX_LINES: usize = 100;
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let read = stream.read_line(&mut line).await.map_err(io_error)?;
        if read == 0 {
            return Err(SmtpError::Network("connection closed".into()));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        let code: u16 = line
            .get(..3)
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| SmtpError::Protocol("malformed reply".into()))?;
        let more = line.as_bytes().get(3) == Some(&b'-');
        lines.push(line.get(4..).unwrap_or_default().to_string());
        if !more {
            return Ok((code, lines));
        }
        if lines.len() > MAX_LINES {
            return Err(SmtpError::Protocol("reply too long".into()));
        }
    }
}

fn io_error(err: std::io::Error) -> SmtpError {
    SmtpError::Network(err.kind().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// A scripted plaintext server: sends `lines` in order, reading one client line before
    /// each entry after the first.
    async fn server(script: Vec<&'static str>) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut sock = BufReader::new(sock);
            let mut got = Vec::new();
            for (i, out) in script.iter().enumerate() {
                if i > 0 {
                    let mut line = String::new();
                    sock.read_line(&mut line).await.unwrap();
                    got.push(line.trim_end().to_string());
                }
                sock.get_mut().write_all(out.as_bytes()).await.unwrap();
            }
            let mut rest = Vec::new();
            let _ = sock.read_to_end(&mut rest).await;
            got
        });
        (port, handle)
    }

    fn config(port: u16) -> SmtpConfig {
        SmtpConfig {
            host: "127.0.0.1".into(),
            port,
            security: Security::StartTls,
            pinned_cert_sha256: None,
            connect_timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn starttls_missing_aborts_before_auth() {
        let (port, handle) = server(vec![
            "220 mail.test ESMTP\r\n",
            "250-mail.test\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 1000\r\n",
        ])
        .await;
        let err = SmtpClient
            .check_login(&config(port), "u", "p")
            .await
            .unwrap_err();
        assert_eq!(err, SmtpError::StartTlsUnavailable);
        let got = handle.await.unwrap();
        assert_eq!(got, vec!["EHLO [127.0.0.1]".to_string()]);
        assert!(
            !got.iter().any(|l| l.starts_with("AUTH")),
            "no plaintext AUTH"
        );
    }

    #[tokio::test]
    async fn starttls_refused_aborts() {
        let (port, handle) = server(vec![
            "220 mail.test ESMTP\r\n",
            "250-mail.test\r\n250 STARTTLS\r\n",
            "454 TLS not available\r\n",
        ])
        .await;
        let err = SmtpClient
            .check_login(&config(port), "u", "p")
            .await
            .unwrap_err();
        assert_eq!(err, SmtpError::StartTlsUnavailable);
        assert_eq!(handle.await.unwrap()[1], "STARTTLS");
    }

    #[tokio::test]
    async fn plaintext_server_on_implicit_tls_port_is_a_tls_error() {
        let (port, _handle) = server(vec!["220 mail.test ESMTP\r\n"]).await;
        let mut cfg = config(port);
        cfg.security = Security::Tls;
        let err = SmtpClient.check_login(&cfg, "u", "p").await.unwrap_err();
        assert!(
            matches!(err, SmtpError::Tls(_) | SmtpError::Network(_)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn refused_port_is_a_network_error() {
        let err = SmtpClient
            .check_login(&config(1), "u", "p")
            .await
            .unwrap_err();
        assert!(matches!(err, SmtpError::Network(_)), "{err:?}");
        assert_eq!(err.class(), ErrorClass::Transient);
    }
}
