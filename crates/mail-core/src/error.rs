//! The UI-facing error (section 3, Command and event contract; codes from SPEC_GAPS G18).
//! Every command returns `Result<T, AppError>`. The UI keys its text off `code`; `message`
//! is detail for that code and never holds a secret or message content.

use mail_auth::AuthError;
use mail_imap::ImapError;
use mail_secrets::SecretsError;
use mail_smtp::SmtpError;
use mail_store::StoreError;
use serde::Serialize;

/// The fixed list of error codes (SPEC_GAPS G18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Network,
    Timeout,
    Certificate,
    Tls,
    AuthFailed,
    Oauth,
    Throttled,
    ServerRejected,
    RecipientRejected,
    TooLarge,
    Unsupported,
    NotFound,
    Conflict,
    InvalidInput,
    Keychain,
    DiskFull,
    DbCorrupt,
    Io,
    Cancelled,
    Internal,
}

/// `{ code, message, account_id? }`. For `certificate`, `message` is the presented leaf
/// certificate's SHA-256 in lowercase hex, which the UI offers to pin (section 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<i64>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            account_id: None,
        }
    }

    pub fn for_account(mut self, account_id: i64) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }
}

impl From<ImapError> for AppError {
    fn from(err: ImapError) -> Self {
        let code = match &err {
            ImapError::Network(_) | ImapError::Bye | ImapError::Protocol(_) => ErrorCode::Network,
            ImapError::Timeout => ErrorCode::Timeout,
            ImapError::Tls(_) | ImapError::StartTlsUnavailable => ErrorCode::Tls,
            ImapError::Certificate { sha256, .. } => {
                return Self::new(ErrorCode::Certificate, sha256.clone());
            }
            ImapError::AuthFailed => ErrorCode::AuthFailed,
            ImapError::Throttled => ErrorCode::Throttled,
            ImapError::No(_) | ImapError::Bad(_) => ErrorCode::ServerRejected,
            ImapError::NoSuchMailbox => ErrorCode::NotFound,
            ImapError::Unsupported(_) => ErrorCode::Unsupported,
            ImapError::NotSelected => ErrorCode::Internal,
        };
        Self::new(code, err.to_string())
    }
}

impl From<SmtpError> for AppError {
    fn from(err: SmtpError) -> Self {
        let code = match &err {
            SmtpError::Network(_) | SmtpError::Unavailable(_) | SmtpError::Protocol(_) => {
                ErrorCode::Network
            }
            SmtpError::Timeout => ErrorCode::Timeout,
            SmtpError::Tls(_) | SmtpError::StartTlsUnavailable => ErrorCode::Tls,
            SmtpError::Certificate { sha256, .. } => {
                return Self::new(ErrorCode::Certificate, sha256.clone());
            }
            SmtpError::AuthFailed => ErrorCode::AuthFailed,
            SmtpError::Rejected(_) => ErrorCode::ServerRejected,
        };
        Self::new(code, err.to_string())
    }
}

impl From<StoreError> for AppError {
    fn from(err: StoreError) -> Self {
        let code = match &err {
            StoreError::DiskFull => ErrorCode::DiskFull,
            StoreError::Corrupt | StoreError::TooNew { .. } => ErrorCode::DbCorrupt,
            StoreError::NotFound => ErrorCode::NotFound,
            StoreError::Invalid(_) => ErrorCode::InvalidInput,
            StoreError::Io(_) | StoreError::NotWal(_) => ErrorCode::Io,
            StoreError::Closed | StoreError::Sqlite(_) | StoreError::Json(_) => ErrorCode::Internal,
        };
        Self::new(code, err.to_string())
    }
}

impl From<SecretsError> for AppError {
    fn from(err: SecretsError) -> Self {
        Self::new(ErrorCode::Keychain, err.to_string())
    }
}

impl From<AuthError> for AppError {
    fn from(err: AuthError) -> Self {
        let code = match &err {
            AuthError::InvalidAddress => ErrorCode::InvalidInput,
            AuthError::MissingSecret => ErrorCode::AuthFailed,
            AuthError::Keychain(_) => ErrorCode::Keychain,
            AuthError::Unsupported(_) => ErrorCode::Unsupported,
        };
        Self::new(code, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_the_contract_shape() {
        let e = AppError::new(ErrorCode::AuthFailed, "authentication failed").for_account(4);
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"code": "auth_failed", "message": "authentication failed", "account_id": 4})
        );
        let e = AppError::new(ErrorCode::DbCorrupt, "x");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"code": "db_corrupt", "message": "x"})
        );
    }

    #[test]
    fn all_twenty_codes_serialize_as_listed_in_g18() {
        use ErrorCode::*;
        let codes = [
            (Network, "network"),
            (Timeout, "timeout"),
            (Certificate, "certificate"),
            (Tls, "tls"),
            (AuthFailed, "auth_failed"),
            (Oauth, "oauth"),
            (Throttled, "throttled"),
            (ServerRejected, "server_rejected"),
            (RecipientRejected, "recipient_rejected"),
            (TooLarge, "too_large"),
            (Unsupported, "unsupported"),
            (NotFound, "not_found"),
            (Conflict, "conflict"),
            (InvalidInput, "invalid_input"),
            (Keychain, "keychain"),
            (DiskFull, "disk_full"),
            (DbCorrupt, "db_corrupt"),
            (Io, "io"),
            (Cancelled, "cancelled"),
            (Internal, "internal"),
        ];
        for (code, text) in codes {
            assert_eq!(serde_json::to_value(code).unwrap(), serde_json::json!(text));
        }
    }

    #[test]
    fn certificate_errors_carry_the_fingerprint() {
        let e: AppError = ImapError::Certificate {
            reason: "UnknownIssuer".into(),
            sha256: "ab".repeat(32),
        }
        .into();
        assert_eq!(e.code, ErrorCode::Certificate);
        assert_eq!(e.message, "ab".repeat(32));
        assert_eq!(
            AppError::from(ImapError::AuthFailed).code,
            ErrorCode::AuthFailed
        );
        assert_eq!(
            AppError::from(SmtpError::AuthFailed).code,
            ErrorCode::AuthFailed
        );
        assert_eq!(
            AppError::from(StoreError::DiskFull).code,
            ErrorCode::DiskFull
        );
        assert_eq!(
            AppError::from(AuthError::MissingSecret).code,
            ErrorCode::AuthFailed
        );
    }
}
