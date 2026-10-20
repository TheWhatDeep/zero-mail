//! Submission (section 7): one SMTP session per message. Every recipient is offered with
//! `RCPT TO`; if any is refused permanently the session ends before `DATA` and the bad
//! addresses are reported, so nothing is ever partly sent. A failure after the message
//! data went out, before the server answered, is ambiguous: the message may have been
//! delivered, and the outbox never retries it (`unknown`).

use std::future::Future;

use lettre::transport::smtp::client::AsyncSmtpConnection;
use lettre::transport::smtp::commands::{Data, Ehlo, Mail, Rcpt};
use lettre::transport::smtp::extension::Extension;
use lettre::Address;

use crate::{client_id, connect, error::from_lettre, step, SmtpConfig, SmtpError, COMMAND_TIMEOUT};

/// How to log in for a submission.
#[derive(Clone, PartialEq, Eq)]
pub enum SmtpLogin {
    /// `AUTH PLAIN`, falling back to `LOGIN`.
    Password { username: String, password: String },
    /// `AUTH XOAUTH2` with an in-memory access token (M8).
    XOAuth2 { username: String, token: String },
}

impl std::fmt::Debug for SmtpLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password { username, .. } => write!(f, "Password({username}, [redacted])"),
            Self::XOAuth2 { username, .. } => write!(f, "XOAuth2({username}, [redacted])"),
        }
    }
}

/// The SMTP envelope: `MAIL FROM` is the identity address; `RCPT TO` is every To, Cc and
/// Bcc address (section 7). The Bcc header is never written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub from: String,
    pub to: Vec<String>,
}

/// Why a submission did not go through, as the outbox needs to know it (section 7).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError {
    /// Network trouble or a 4xx reply before the message was accepted: retry later.
    #[error("temporary failure: {0}")]
    Transient(SmtpError),
    /// One or more recipients refused permanently; nothing was sent (no DATA).
    #[error("recipients refused")]
    RecipientsRejected(Vec<String>),
    /// Over the server's size limit (`552`, or `SIZE`).
    #[error("message too large")]
    TooLarge,
    /// Wrong password or revoked token.
    #[error("authentication failed")]
    AuthFailed,
    /// Any other permanent refusal (5xx), or a certificate or TLS failure.
    #[error("refused: {0}")]
    Permanent(SmtpError),
    /// The connection failed after the data was sent and before the reply: it may have
    /// been delivered. Never retried automatically.
    #[error("the server's answer was lost")]
    Ambiguous,
}

/// Sends messages. Real servers use [`crate::SmtpClient`]; tests use the fake.
pub trait SmtpSubmit: Send + Sync {
    /// One session: connect, authenticate, envelope, data, quit.
    fn submit(
        &self,
        config: &SmtpConfig,
        login: &SmtpLogin,
        envelope: &Envelope,
        raw: &[u8],
    ) -> impl Future<Output = Result<(), SubmitError>> + Send;

    /// The server's `SIZE` limit from `EHLO`, if it announces one (section 7: checked
    /// before queueing).
    fn size_limit(
        &self,
        config: &SmtpConfig,
    ) -> impl Future<Output = Result<Option<u64>, SmtpError>> + Send;
}

fn address(text: &str) -> Result<Address, SubmitError> {
    text.parse::<Address>()
        .map_err(|_| SubmitError::Permanent(SmtpError::Rejected(format!("invalid address {text}"))))
}

/// A refusal of `MAIL FROM` or `DATA`.
fn refusal(err: SmtpError) -> SubmitError {
    match err {
        SmtpError::Rejected(code) if code.starts_with("552") => SubmitError::TooLarge,
        SmtpError::Rejected(_) => SubmitError::Permanent(err),
        SmtpError::AuthFailed => SubmitError::AuthFailed,
        e if e.class() == crate::ErrorClass::NeedsAttention => SubmitError::Permanent(e),
        e => SubmitError::Transient(e),
    }
}

pub(crate) async fn authenticate(
    conn: &mut AsyncSmtpConnection,
    login: &SmtpLogin,
) -> Result<(), SmtpError> {
    use lettre::transport::smtp::authentication::{Credentials, Mechanism};
    let (credentials, mechanisms): (Credentials, &[Mechanism]) = match login {
        SmtpLogin::Password { username, password } => (
            Credentials::new(username.clone(), password.clone()),
            &[Mechanism::Plain, Mechanism::Login],
        ),
        SmtpLogin::XOAuth2 { username, token } => (
            Credentials::new(username.clone(), token.clone()),
            &[Mechanism::Xoauth2],
        ),
    };
    match step(conn.auth(mechanisms, &credentials)).await {
        Ok(_) => Ok(()),
        Err(SmtpError::Rejected(_)) => Err(SmtpError::AuthFailed),
        Err(e) => Err(e),
    }
}

pub(crate) async fn submit(
    config: &SmtpConfig,
    login: &SmtpLogin,
    envelope: &Envelope,
    raw: &[u8],
) -> Result<(), SubmitError> {
    let from = address(&envelope.from)?;
    let to: Vec<(String, Address)> = envelope
        .to
        .iter()
        .map(|t| Ok((t.clone(), address(t)?)))
        .collect::<Result<_, SubmitError>>()?;
    let mut conn = connect(config).await.map_err(refusal)?;
    if let Err(e) = authenticate(&mut conn, login).await {
        conn.abort().await;
        return Err(refusal(e));
    }
    let utf8 = envelope
        .to
        .iter()
        .chain(std::iter::once(&envelope.from))
        .any(|a| !a.is_ascii());
    let mut params = Vec::new();
    if utf8 {
        if !conn.server_info().supports_feature(Extension::SmtpUtfEight) {
            conn.abort().await;
            return Err(SubmitError::Permanent(SmtpError::Rejected(
                "the server does not accept international addresses".into(),
            )));
        }
        params.push(lettre::transport::smtp::extension::MailParameter::SmtpUtfEight);
    }
    if let Err(e) = step(conn.command(Mail::new(Some(from), params))).await {
        conn.abort().await;
        return Err(refusal(e));
    }
    let mut rejected = Vec::new();
    for (text, addr) in to {
        match step(conn.command(Rcpt::new(addr, Vec::new()))).await {
            Ok(_) => {}
            Err(SmtpError::Rejected(_)) => rejected.push(text),
            Err(e) => {
                conn.abort().await;
                return Err(SubmitError::Transient(e));
            }
        }
    }
    if !rejected.is_empty() {
        // No partial sends: stop before DATA (section 7, T15).
        conn.abort().await;
        return Err(SubmitError::RecipientsRejected(rejected));
    }
    if let Err(e) = step(conn.command(Data)).await {
        conn.abort().await;
        return Err(refusal(e));
    }
    let sent = tokio::time::timeout(crate::DATA_TIMEOUT, conn.message(raw)).await;
    match sent {
        Ok(Ok(_)) => {
            let _ = step(conn.quit()).await;
            Ok(())
        }
        // The server answered: a clear refusal or a temporary failure.
        Ok(Err(e)) if e.is_permanent() || e.is_transient() => Err(refusal(from_lettre(e))),
        // The connection broke or timed out after the data went out.
        Ok(Err(_)) | Err(_) => {
            conn.abort().await;
            Err(SubmitError::Ambiguous)
        }
    }
}

/// `SIZE` from the server's `EHLO` reply.
pub(crate) async fn size_limit(config: &SmtpConfig) -> Result<Option<u64>, SmtpError> {
    let mut conn = connect(config).await?;
    let reply = step(conn.command(Ehlo::new(client_id()))).await;
    let _ = tokio::time::timeout(COMMAND_TIMEOUT, conn.quit()).await;
    let reply = reply?;
    let size = reply.message().find_map(parse_size);
    Ok(size)
}

/// `SIZE n` from one `EHLO` line; `SIZE` alone or `SIZE 0` means no fixed limit.
fn parse_size(line: &str) -> Option<u64> {
    let mut words = line.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("SIZE") {
        return None;
    }
    words.next()?.parse::<u64>().ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_lines() {
        assert_eq!(parse_size("SIZE 35882577"), Some(35_882_577));
        assert_eq!(parse_size("size 10"), Some(10));
        assert_eq!(parse_size("SIZE"), None);
        assert_eq!(parse_size("SIZE 0"), None);
        assert_eq!(parse_size("8BITMIME"), None);
    }
}
