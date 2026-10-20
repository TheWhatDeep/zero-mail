#![forbid(unsafe_code)]
//! `auth` (BUILD.md section 3): password and XOAUTH2 login, token refresh, the OAuth flow,
//! and server-settings resolution (SPEC_GAPS G29). May call only `secrets`.
//!
//! - [`resolve`]: built-in table, the domain's autoconfig file, the Mozilla ISP database,
//!   then nothing (the UI shows the manual form). Section 5, step 2.
//! - [`auth_methods`]: OAuth for Google hosts, password for everything else (step 3).
//! - [`password_login`]: the account's password from the keychain, for one login.
//! - [`oauth`]: Google sign-in (M8). [`KeychainLogins`] hands out access tokens that live in
//!   memory only and are refreshed 5 minutes before expiry (section 5, Tokens).
//!
//! This crate knows nothing of the IMAP or SMTP crates (section 3); the caller turns a
//! [`Login`] into protocol credentials.

mod autoconfig;
mod builtin;
mod http;
pub mod oauth;
mod xml;

pub use autoconfig::{resolve, HttpGet, AUTOCONFIG_MAX_BYTES};
pub use http::HttpClient;
pub use mail_secrets::{Secret, SecretStore};
pub use oauth::{GoogleClient, HttpFailure, HttpPost, HttpReply, Tokens};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use mail_secrets::{key, SecretKind, SecretsError};

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
    /// Google's token endpoint could not be reached or answered with a server error. Tried
    /// again later, like any network trouble.
    #[error("Google's token endpoint is unreachable: {0}")]
    Network(String),
    /// Google refused the grant (`invalid_grant`: revoked or expired) or the client. Only
    /// signing in again fixes it (section 5, Tokens).
    #[error("Google refused the sign-in: {0}")]
    Rejected(String),
    /// The browser sign-in failed, was denied or timed out.
    #[error("{0}")]
    OAuth(String),
}

impl AuthError {
    /// Whether waiting and trying again may help: the account goes to Backoff, not
    /// `NeedsAttention`.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Network(_))
    }
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

/// Who is logging in, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRequest {
    pub account_id: i64,
    pub username: String,
    pub method: AuthMethod,
    /// The user's OAuth client ID; empty for password accounts.
    pub client_id: String,
}

/// Hands out credentials for one login. The sync actors and the outbox take their logins
/// from here (section 3: `sync` calls `auth`, which alone calls `secrets`).
pub trait LoginSource: Send + Sync + 'static {
    fn login(
        &self,
        request: &LoginRequest,
    ) -> impl std::future::Future<Output = Result<Login, AuthError>> + Send;
}

/// One account's access token and its expiry, in memory only (section 13, item 2).
type TokenSlot = Option<(Secret, tokio::time::Instant)>;

/// Logins from the OS keychain: the account's app password, or for Google an access token
/// obtained with the refresh token in the keychain. Access tokens stay in memory and are
/// refreshed 5 minutes before expiry; one refresh at a time runs per account.
pub struct KeychainLogins<H> {
    secrets: Arc<dyn SecretStore>,
    http: H,
    tokens: Arc<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<TokenSlot>>>>>,
}

impl<H: Clone> Clone for KeychainLogins<H> {
    fn clone(&self) -> Self {
        Self {
            secrets: Arc::clone(&self.secrets),
            http: self.http.clone(),
            tokens: Arc::clone(&self.tokens),
        }
    }
}

impl<H> std::fmt::Debug for KeychainLogins<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeychainLogins").finish_non_exhaustive()
    }
}

impl<H: HttpPost + Clone + 'static> KeychainLogins<H> {
    pub fn new(secrets: Arc<dyn SecretStore>, http: H) -> Self {
        Self {
            secrets,
            http,
            tokens: Arc::default(),
        }
    }

    /// The HTTP client for Google's token endpoint.
    pub fn http(&self) -> &H {
        &self.http
    }

    fn slot(&self, account_id: i64) -> Arc<tokio::sync::Mutex<TokenSlot>> {
        let mut map = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(map.entry(account_id).or_default())
    }

    /// The account's Google client: the ID saved with the account, the secret from the
    /// keychain (SPEC_GAPS G70).
    pub async fn google_client(
        &self,
        account_id: i64,
        client_id: &str,
    ) -> Result<GoogleClient, AuthError> {
        let client_secret = self
            .read(account_id, SecretKind::OauthClientSecret)
            .await?
            .ok_or(AuthError::MissingSecret)?;
        Ok(GoogleClient {
            client_id: client_id.to_string(),
            client_secret,
        })
    }

    /// After a browser sign-in: the refresh token goes to the keychain and the access token
    /// stays in memory.
    pub async fn signed_in(&self, account_id: i64, tokens: Tokens) -> Result<(), AuthError> {
        let slot = self.slot(account_id);
        let mut slot = slot.lock().await;
        if let Some(refresh) = tokens.refresh {
            self.write(account_id, SecretKind::RefreshToken, refresh)
                .await?;
        }
        *slot = Some((tokens.access, tokens.expires_at));
        Ok(())
    }

    /// Drops the account's access token (account removed).
    pub fn forget(&self, account_id: i64) {
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&account_id);
    }

    async fn access_token(&self, request: &LoginRequest) -> Result<Secret, AuthError> {
        let slot = self.slot(request.account_id);
        let mut slot = slot.lock().await;
        if let Some((token, expires_at)) = slot.as_ref() {
            if !oauth::needs_refresh(*expires_at, tokio::time::Instant::now()) {
                return Ok(token.clone());
            }
        }
        let refresh_token = self
            .read(request.account_id, SecretKind::RefreshToken)
            .await?
            .ok_or(AuthError::MissingSecret)?;
        let client = self
            .google_client(request.account_id, &request.client_id)
            .await?;
        match oauth::refresh(&client, &self.http, &refresh_token).await {
            Ok(tokens) => {
                if let Some(refresh) = tokens.refresh {
                    self.write(request.account_id, SecretKind::RefreshToken, refresh)
                        .await?;
                }
                *slot = Some((tokens.access.clone(), tokens.expires_at));
                Ok(tokens.access)
            }
            Err(e) => {
                *slot = None;
                Err(e)
            }
        }
    }

    async fn read(&self, account_id: i64, kind: SecretKind) -> Result<Option<Secret>, AuthError> {
        let secrets = Arc::clone(&self.secrets);
        tokio::task::spawn_blocking(move || secrets.get(&key(account_id, kind)))
            .await
            .map_err(|_| AuthError::Unsupported("keychain access"))?
            .map_err(AuthError::from)
    }

    async fn write(
        &self,
        account_id: i64,
        kind: SecretKind,
        value: Secret,
    ) -> Result<(), AuthError> {
        let secrets = Arc::clone(&self.secrets);
        tokio::task::spawn_blocking(move || secrets.set(&key(account_id, kind), &value))
            .await
            .map_err(|_| AuthError::Unsupported("keychain access"))?
            .map_err(AuthError::from)
    }
}

impl<H: HttpPost + Clone + 'static> LoginSource for KeychainLogins<H> {
    async fn login(&self, request: &LoginRequest) -> Result<Login, AuthError> {
        match request.method {
            AuthMethod::Password => {
                let secrets = Arc::clone(&self.secrets);
                let (id, username) = (request.account_id, request.username.clone());
                tokio::task::spawn_blocking(move || password_login(secrets.as_ref(), id, &username))
                    .await
                    .map_err(|_| AuthError::Unsupported("keychain access"))?
            }
            AuthMethod::OauthGoogle => Ok(Login {
                username: request.username.clone(),
                secret: self.access_token(request).await?,
                method: AuthMethod::OauthGoogle,
            }),
        }
    }
}

/// Saves or replaces an account's password. Blocking (keychain).
pub fn store_password(
    secrets: &dyn SecretStore,
    account_id: i64,
    password: &Secret,
) -> Result<(), AuthError> {
    Ok(secrets.set(&key(account_id, SecretKind::Password), password)?)
}

/// Saves or replaces an account's Google client secret (SPEC_GAPS G70). Blocking.
pub fn store_client_secret(
    secrets: &dyn SecretStore,
    account_id: i64,
    secret: &Secret,
) -> Result<(), AuthError> {
    Ok(secrets.set(&key(account_id, SecretKind::OauthClientSecret), secret)?)
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

    /// A token endpoint that answers every refresh with a numbered token and the given
    /// lifetime, and counts the calls.
    #[derive(Debug, Clone, Default)]
    struct Counting {
        calls: Arc<Mutex<u32>>,
        lifetime: u64,
        refuse: bool,
    }

    impl HttpPost for Counting {
        async fn post_form(&self, _url: &str, _body: Vec<u8>) -> Result<HttpReply, HttpFailure> {
            let mut n = self.calls.lock().unwrap();
            *n += 1;
            let body = if self.refuse {
                return Ok(HttpReply {
                    status: 400,
                    body: br#"{"error":"invalid_grant"}"#.to_vec(),
                });
            } else {
                format!(
                    r#"{{"access_token":"at-{n}","expires_in":{},"token_type":"Bearer"}}"#,
                    self.lifetime
                )
            };
            Ok(HttpReply {
                status: 200,
                body: body.into_bytes(),
            })
        }
    }

    fn google_request() -> LoginRequest {
        LoginRequest {
            account_id: 9,
            username: "me@example.com".into(),
            method: AuthMethod::OauthGoogle,
            client_id: "cid".into(),
        }
    }

    #[tokio::test]
    async fn google_logins_refresh_only_near_expiry_and_keep_tokens_in_memory() {
        let secrets = Arc::new(MemoryStore::default());
        let http = Counting {
            lifetime: 3599,
            ..Counting::default()
        };
        let logins = KeychainLogins::new(secrets.clone(), http.clone());
        // Not signed in yet: no refresh token, and nothing is asked of Google.
        assert_eq!(
            logins.login(&google_request()).await,
            Err(AuthError::MissingSecret)
        );
        store_client_secret(secrets.as_ref(), 9, &Secret::new("cs")).unwrap();
        assert_eq!(
            logins.login(&google_request()).await,
            Err(AuthError::MissingSecret)
        );
        assert_eq!(*http.calls.lock().unwrap(), 0);
        secrets
            .set(&key(9, SecretKind::RefreshToken), &Secret::new("rt"))
            .unwrap();
        let first = logins.login(&google_request()).await.unwrap();
        let second = logins.login(&google_request()).await.unwrap();
        assert_eq!(first.secret.expose(), "at-1");
        assert_eq!(second.secret.expose(), "at-1", "an hour left: no refresh");
        assert_eq!(first.method, AuthMethod::OauthGoogle);
        assert_eq!(*http.calls.lock().unwrap(), 1);
        // The keychain holds the client secret and the refresh token, never an access token.
        assert_eq!(
            secrets.keys(),
            vec![
                "account:9:oauth_client_secret".to_string(),
                "account:9:refresh_token".to_string()
            ]
        );

        // A token with 4 minutes left is inside the 5 minute margin: every login refreshes.
        let short = Counting {
            lifetime: 240,
            ..Counting::default()
        };
        let logins = KeychainLogins::new(secrets.clone(), short.clone());
        assert_eq!(
            logins
                .login(&google_request())
                .await
                .unwrap()
                .secret
                .expose(),
            "at-1"
        );
        assert_eq!(
            logins
                .login(&google_request())
                .await
                .unwrap()
                .secret
                .expose(),
            "at-2"
        );
    }

    #[tokio::test]
    async fn a_revoked_grant_is_rejected_and_the_cached_token_dropped() {
        let secrets = Arc::new(MemoryStore::default());
        store_client_secret(secrets.as_ref(), 9, &Secret::new("cs")).unwrap();
        let logins = KeychainLogins::new(
            secrets.clone(),
            Counting {
                refuse: true,
                ..Counting::default()
            },
        );
        logins
            .signed_in(
                9,
                Tokens {
                    access: Secret::new("old"),
                    expires_at: tokio::time::Instant::now(),
                    refresh: Some(Secret::new("rt")),
                },
            )
            .await
            .unwrap();
        let err = logins.login(&google_request()).await.unwrap_err();
        assert_eq!(err, AuthError::Rejected("invalid_grant".into()));
        assert!(!err.is_transient());
        assert!(AuthError::Network("x".into()).is_transient());
    }
}
