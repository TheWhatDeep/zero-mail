/// Every failure an IMAP operation can report. Messages never contain credentials or
/// message content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImapError {
    #[error("network error: {0}")]
    Network(String),
    #[error("operation timed out")]
    Timeout,
    #[error("TLS handshake failed: {0}")]
    Tls(String),
    /// Invalid, untrusted or changed certificate. `sha256` is the presented leaf
    /// certificate's fingerprint in lowercase hex, so the UI can offer pinning.
    #[error("server certificate rejected: {reason}")]
    Certificate { reason: String, sha256: String },
    #[error("server does not offer STARTTLS")]
    StartTlsUnavailable,
    #[error("authentication failed")]
    AuthFailed,
    /// `[LIMIT]`, `[UNAVAILABLE]` or a provider bandwidth message (section 6).
    #[error("server is throttling this account")]
    Throttled,
    #[error("server closed the connection")]
    Bye,
    #[error("server refused the command: {0}")]
    No(String),
    #[error("server rejected the command syntax: {0}")]
    Bad(String),
    #[error("no such mailbox")]
    NoSuchMailbox,
    /// The operation needs a capability the server does not advertise.
    #[error("server does not support {0}")]
    Unsupported(&'static str),
    #[error("malformed server response: {0}")]
    Protocol(String),
    /// A command was issued that needs a selected folder.
    #[error("no folder selected")]
    NotSelected,
}

/// How the sync actor reacts to an error (sections 6 and 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Drop the connection and go to Backoff.
    Transient,
    /// Pause the account for a fixed 10 minutes.
    Throttle,
    /// Stop the account until the user fixes it (bad login or certificate).
    NeedsAttention,
    /// The command failed but the connection is fine; revert or report the single op.
    Permanent,
}

impl ImapError {
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Network(_) | Self::Timeout | Self::Bye | Self::Protocol(_) => {
                ErrorClass::Transient
            }
            Self::Throttled => ErrorClass::Throttle,
            Self::Tls(_)
            | Self::Certificate { .. }
            | Self::StartTlsUnavailable
            | Self::AuthFailed => ErrorClass::NeedsAttention,
            Self::No(_)
            | Self::Bad(_)
            | Self::NoSuchMailbox
            | Self::Unsupported(_)
            | Self::NotSelected => ErrorClass::Permanent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_match_section_12() {
        assert_eq!(ImapError::Timeout.class(), ErrorClass::Transient);
        assert_eq!(ImapError::Bye.class(), ErrorClass::Transient);
        assert_eq!(ImapError::Throttled.class(), ErrorClass::Throttle);
        assert_eq!(ImapError::AuthFailed.class(), ErrorClass::NeedsAttention);
        let cert = ImapError::Certificate {
            reason: "changed".into(),
            sha256: "ab".into(),
        };
        assert_eq!(cert.class(), ErrorClass::NeedsAttention);
        assert_eq!(
            ImapError::StartTlsUnavailable.class(),
            ErrorClass::NeedsAttention
        );
        assert_eq!(ImapError::No("x".into()).class(), ErrorClass::Permanent);
    }
}
