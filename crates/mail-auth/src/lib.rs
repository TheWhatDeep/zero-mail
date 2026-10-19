#![forbid(unsafe_code)]
//! `auth` (BUILD.md section 3): password and XOAUTH2 login, token refresh, the OAuth flow,
//! and server-settings resolution (SPEC_GAPS G29). May call only `secrets`.
//!
//! M2 builds the password half and resolution:
//! - [`resolve`]: built-in table, the domain's autoconfig file, the Mozilla ISP database,
//!   then nothing (the UI shows the manual form). Section 5, step 2.
//! - [`auth_methods`]: OAuth for Google hosts, password for everything else (step 3).
//! - [`password_login`]: the account's password from the keychain, for one login.
//!
//! This crate knows nothing of the IMAP or SMTP crates (section 3); the caller turns a
//! [`Login`] into protocol credentials.

mod autoconfig;
mod builtin;
mod http;
mod xml;

pub use autoconfig::{resolve, HttpGet, AUTOCONFIG_MAX_BYTES};
pub use http::HttpClient;
pub use mail_secrets::Secret;

use mail_secrets::{key, SecretKind, SecretStore, SecretsError};

/// Transport security for one endpoint. There is no plaintext option (section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Tls,
    StartTls,
}

/// One server endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

/// Where a candidate came from (section 5, step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    BuiltIn,
    Autoconfig,
    IspDb,
}

/// One server-settings candidate for an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub username: String,
    pub source: Source,
}

/// A login method (section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    Password,
    OauthGoogle,
}

/// Credentials for exactly one login. The secret never prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub username: String,
    pub secret: Secret,
    pub method: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("not a valid email address")]
    InvalidAddress,
    /// The keychain has no secret for the account; the user signs in again.
    #[error("no saved password for this account")]
    MissingSecret,
    #[error(transparent)]
    Keychain(#[from] SecretsError),
    #[error("{0} is not available yet")]
    Unsupported(&'static str),
}

/// Login methods offered for an IMAP host (section 5, step 3): Google hosts offer OAuth, and
/// also an app password, which personal gmail.com accounts need (section 5, Bring your own
/// OAuth client). Every other host offers a password.
pub fn auth_methods(imap_host: &str) -> Vec<AuthMethod> {
    if is_google_host(imap_host) {
        vec![AuthMethod::OauthGoogle, AuthMethod::Password]
    } else {
        vec![AuthMethod::Password]
    }
}

fn is_google_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ["gmail.com", "googlemail.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// The saved password of an account, for one login. Blocking (keychain).
pub fn password_login(
    secrets: &dyn SecretStore,
    account_id: i64,
    username: &str,
) -> Result<Login, AuthError> {
    let secret = secrets
        .get(&key(account_id, SecretKind::Password))?
        .ok_or(AuthError::MissingSecret)?;
    Ok(Login {
        username: username.to_string(),
        secret,
        method: AuthMethod::Password,
    })
}

/// Saves or replaces an account's password. Blocking (keychain).
pub fn store_password(
    secrets: &dyn SecretStore,
    account_id: i64,
    password: &Secret,
) -> Result<(), AuthError> {
    Ok(secrets.set(&key(account_id, SecretKind::Password), password)?)
}

/// Splits an address into its local part and lowercase domain.
pub fn split_address(email: &str) -> Result<(&str, String), AuthError> {
    let email = email.trim();
    let (local, domain) = email.rsplit_once('@').ok_or(AuthError::InvalidAddress)?;
    let domain = domain.trim_end_matches('.').to_lowercase();
    let valid_domain = !domain.is_empty()
        && domain.contains('.')
        && domain.len() <= 253
        && domain
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '.')
        && !domain.starts_with(['.', '-'])
        && !domain.contains("..");
    let valid_local =
        !local.is_empty() && local.len() <= 64 && !local.contains(char::is_whitespace);
    if valid_domain && valid_local {
        Ok((local, domain))
    } else {
        Err(AuthError::InvalidAddress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_secrets::MemoryStore;

    #[test]
    fn google_hosts_offer_oauth_and_app_passwords() {
        assert_eq!(
            auth_methods("imap.gmail.com"),
            vec![AuthMethod::OauthGoogle, AuthMethod::Password]
        );
        assert_eq!(
            auth_methods("IMAP.GoogleMail.com."),
            auth_methods("imap.gmail.com")
        );
        assert_eq!(
            auth_methods("imap.fastmail.com"),
            vec![AuthMethod::Password]
        );
        assert_eq!(auth_methods("notgmail.com"), vec![AuthMethod::Password]);
    }

    #[test]
    fn addresses() {
        assert_eq!(
            split_address(" Deep@Example.COM ").unwrap(),
            ("Deep", "example.com".to_string())
        );
        assert_eq!(
            split_address("a@münchen.de").unwrap().1,
            "münchen.de".to_string()
        );
        for bad in [
            "",
            "x",
            "@a.com",
            "a@",
            "a@b",
            "a@b/c.com",
            "a@b..com",
            "a@.b.com",
            "a b@c.com",
        ] {
            assert_eq!(split_address(bad), Err(AuthError::InvalidAddress), "{bad}");
        }
    }

    #[test]
    fn passwords_come_from_the_keychain() {
        let secrets = MemoryStore::default();
        assert_eq!(
            password_login(&secrets, 3, "u"),
            Err(AuthError::MissingSecret)
        );
        store_password(&secrets, 3, &Secret::new("pw")).unwrap();
        let login = password_login(&secrets, 3, "u").unwrap();
        assert_eq!(login.secret.expose(), "pw");
        assert_eq!(login.method, AuthMethod::Password);
        assert!(!format!("{login:?}").contains("pw\""));
        assert_eq!(secrets.keys(), vec!["account:3:password".to_string()]);
    }
}
