#![forbid(unsafe_code)]
//! `secrets` (BUILD.md section 3): keychain read and write. Calls no other module.
//!
//! Secrets live only in the OS keychain (section 13, item 2): Windows Credential Manager,
//! macOS Keychain, or the Secret Service on Linux. Entries are keyed by account id, for
//! example `account:7:password` (section 5), under the service name of the app identifier.
//! [`Secret`] never prints, so a secret cannot reach a log or an error message by accident.
//!
//! The keychain API is blocking; async callers run these methods on a blocking thread.

#[cfg(any(test, feature = "fake"))]
mod memory;

#[cfg(any(test, feature = "fake"))]
pub use memory::MemoryStore;

use std::fmt;

/// The keychain service name: the app identifier (SPEC_GAPS G2).
pub const SERVICE: &str = "com.digitalcircuitry.zeromail";

/// A secret value. `Debug` and `Display` show `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read the value. Call sites are easy to audit.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// What a keychain entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// App password (section 5).
    Password,
    /// OAuth refresh token (section 5, Tokens; M8).
    RefreshToken,
    /// The client secret of the user's own Google Desktop-app client (SPEC_GAPS G70).
    OauthClientSecret,
}

impl SecretKind {
    pub const ALL: [SecretKind; 3] = [
        SecretKind::Password,
        SecretKind::RefreshToken,
        SecretKind::OauthClientSecret,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::RefreshToken => "refresh_token",
            Self::OauthClientSecret => "oauth_client_secret",
        }
    }
}

/// The keychain key for one secret of one account: `account:<id>:<kind>`.
pub fn key(account_id: i64, kind: SecretKind) -> String {
    format!("account:{account_id}:{}", kind.as_str())
}

/// Keychain failures. Messages never contain the secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    /// No usable keychain on this system, or it refused access.
    #[error("the OS keychain is unavailable: {0}")]
    Unavailable(String),
    /// The keychain rejected the value or the key.
    #[error("the OS keychain rejected the entry: {0}")]
    Rejected(String),
}

/// A place to keep secrets. [`Keychain`] in the app; [`MemoryStore`] in tests.
pub trait SecretStore: Send + Sync + fmt::Debug {
    fn set(&self, key: &str, value: &Secret) -> Result<(), SecretsError>;

    /// `Ok(None)` when no entry exists.
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretsError>;

    /// Deleting a missing entry is not an error.
    fn delete(&self, key: &str) -> Result<(), SecretsError>;

    /// Removes every secret of an account (section 5: removing an account deletes its
    /// keychain entries).
    fn delete_account(&self, account_id: i64) -> Result<(), SecretsError> {
        for kind in SecretKind::ALL {
            self.delete(&key(account_id, kind))?;
        }
        Ok(())
    }
}

/// The OS keychain, through the `keyring` crate.
#[derive(Debug, Clone)]
pub struct Keychain {
    service: String,
}

impl Keychain {
    /// The app's keychain, under [`SERVICE`].
    pub fn new() -> Self {
        Self::with_service(SERVICE)
    }

    /// A keychain under another service name. Tests use a unique one.
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, SecretsError> {
        keyring::Entry::new(&self.service, key).map_err(map_err)
    }
}

impl Default for Keychain {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for Keychain {
    fn set(&self, key: &str, value: &Secret) -> Result<(), SecretsError> {
        self.entry(key)?
            .set_password(value.expose())
            .map_err(map_err)
    }

    fn get(&self, key: &str) -> Result<Option<Secret>, SecretsError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(map_err(err)),
        }
    }

    fn delete(&self, key: &str) -> Result<(), SecretsError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(map_err(err)),
        }
    }
}

/// Maps a keyring error to a message that names the failure, never the value.
fn map_err(err: keyring::Error) -> SecretsError {
    match err {
        keyring::Error::BadEncoding(_) => SecretsError::Rejected("not valid UTF-8".into()),
        keyring::Error::TooLong(field, limit) => {
            SecretsError::Rejected(format!("{field} is longer than {limit}"))
        }
        keyring::Error::Invalid(field, reason) => {
            SecretsError::Rejected(format!("invalid {field}: {reason}"))
        }
        other => SecretsError::Unavailable(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_section_5() {
        assert_eq!(key(7, SecretKind::Password), "account:7:password");
        assert_eq!(key(7, SecretKind::RefreshToken), "account:7:refresh_token");
        assert_eq!(
            key(7, SecretKind::OauthClientSecret),
            "account:7:oauth_client_secret"
        );
    }

    #[test]
    fn secrets_never_print() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s} {s:?}"), "[redacted] [redacted]");
    }

    #[test]
    fn memory_store_round_trip_and_account_delete() {
        let store = MemoryStore::default();
        let k = key(1, SecretKind::Password);
        assert_eq!(store.get(&k).unwrap(), None);
        store.set(&k, &Secret::new("one")).unwrap();
        store.set(&k, &Secret::new("two")).unwrap();
        assert_eq!(store.get(&k).unwrap(), Some(Secret::new("two")));
        store
            .set(&key(1, SecretKind::RefreshToken), &Secret::new("rt"))
            .unwrap();
        store
            .set(&key(2, SecretKind::Password), &Secret::new("other"))
            .unwrap();
        store.delete_account(1).unwrap();
        assert_eq!(store.get(&k).unwrap(), None);
        assert_eq!(store.get(&key(1, SecretKind::RefreshToken)).unwrap(), None);
        assert!(store.get(&key(2, SecretKind::Password)).unwrap().is_some());
        store.delete(&k).unwrap();
    }

    /// Round trip through the real OS keychain under a throwaway service name. Builds and
    /// tests run on Windows (SPEC_GAPS G24); the Linux runner has no Secret Service.
    #[test]
    #[cfg_attr(
        not(any(windows, target_os = "macos")),
        ignore = "needs an OS keychain"
    )]
    fn os_keychain_round_trip() {
        let service = format!("{SERVICE}.test-{}", std::process::id());
        let keychain = Keychain::with_service(service);
        let k = key(42, SecretKind::Password);
        keychain.delete(&k).unwrap();
        assert_eq!(keychain.get(&k).unwrap(), None);
        keychain.set(&k, &Secret::new("p4ss wörd")).unwrap();
        assert_eq!(keychain.get(&k).unwrap(), Some(Secret::new("p4ss wörd")));
        keychain.delete_account(42).unwrap();
        assert_eq!(keychain.get(&k).unwrap(), None);
    }
}
