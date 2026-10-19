/// Every failure an SMTP operation can report. Messages never contain credentials or
/// message content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmtpError {
    #[error("network error: {0}")]
    Network(String),
    #[error("operation timed out")]
    Timeout,
    #[error("TLS handshake failed: {0}")]
    Tls(String),
    /// Invalid, untrusted or changed certificate, with the presented leaf certificate's
    /// SHA-256 in lowercase hex so the UI can offer pinning.
    #[error("server certificate rejected: {reason}")]
    Certificate { reason: String, sha256: String },
    #[error("server does not offer STARTTLS")]
    StartTlsUnavailable,
    #[error("authentication failed")]
    AuthFailed,
    /// A transient 4xx reply.
    #[error("server is temporarily unavailable: {0}")]
    Unavailable(String),
    /// A permanent 5xx reply.
    #[error("server rejected the command: {0}")]
    Rejected(String),
    #[error("malformed server reply: {0}")]
    Protocol(String),
}

/// How a caller reacts, matching the IMAP classes (sections 6 and 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Transient,
    NeedsAttention,
    Permanent,
}

impl SmtpError {
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Network(_) | Self::Timeout | Self::Unavailable(_) | Self::Protocol(_) => {
                ErrorClass::Transient
            }
            Self::Tls(_)
            | Self::Certificate { .. }
            | Self::StartTlsUnavailable
            | Self::AuthFailed => ErrorClass::NeedsAttention,
            Self::Rejected(_) => ErrorClass::Permanent,
        }
    }
}

pub(crate) fn from_lettre(err: lettre::transport::smtp::Error) -> SmtpError {
    let code = err.status().map(|c| c.to_string()).unwrap_or_default();
    if err.is_timeout() {
        SmtpError::Timeout
    } else if err.is_permanent() {
        SmtpError::Rejected(code)
    } else if err.is_transient() {
        SmtpError::Unavailable(code)
    } else if err.is_client() {
        // For example: no common AUTH mechanism.
        SmtpError::Rejected("no supported authentication mechanism".into())
    } else if err.is_response() {
        SmtpError::Protocol("malformed reply".into())
    } else {
        SmtpError::Network("connection failed".into())
    }
}
