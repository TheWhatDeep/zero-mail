//! Account CRUD (section 5): `account.resolve`, `account.test`, `account.add`,
//! `account.update`, `account.remove`, `account.set_enabled`, and the account list.
//!
//! The password arrives once from the UI, goes straight to the keychain under
//! `account:<id>:password`, and is never returned (section 13, item 2). An account saves
//! only when both halves of Test connection pass (IMAP login plus `LIST`, SMTP `EHLO` plus
//! `AUTH`), unless the user picks Save anyway (section 5, step 4).
//!
//! A Google sign-in account is saved first and signed in second (SPEC_GAPS G71). `add`
//! keeps its client ID in the row and its client secret in the keychain (G70), and runs no
//! test. The test runs after the browser sign-in ([`Accounts::test_saved`]).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mail_auth::{AuthMethod, HttpGet, Secret};
use mail_imap::{Capabilities, ConnectConfig, Credentials, ImapConnector, ImapTransport};
use mail_secrets::SecretStore;
use mail_smtp::{SmtpCheck, SmtpConfig, SmtpLogin};
use mail_store::{AccountId, NewAccount, NewIdentity, SaveSentCopy, Store};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{AppError, ErrorCode};

/// Setting key: online autoconfig lookups (section 5, step 2). Default on.
pub const ONLINE_LOOKUPS: &str = "privacy.online_lookups";

/// Default sync window in days (section 4, `sync_days DEFAULT 90`).
const DEFAULT_SYNC_DAYS: u32 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    Tls,
    Starttls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Password,
    OauthGoogle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    BuiltIn,
    Autoconfig,
    IspDb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

/// What the setup or edit form sends.
#[derive(Debug, Clone, Deserialize)]
pub struct AccountConfig {
    pub name: String,
    pub email: String,
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub username: String,
    pub auth_kind: AuthKind,
    /// Handled in Core only; never echoed back. On update, `None` keeps the saved one.
    #[serde(default, deserialize_with = "secret")]
    pub password: Option<Secret>,
    #[serde(default)]
    pub pinned_cert_sha256: Option<String>,
    /// Save even if Test connection fails (section 5, step 4).
    #[serde(default)]
    pub save_anyway: bool,
    /// The account's signature (section 11, Identities and signatures; SPEC_GAPS G64).
    /// `None` keeps the saved one.
    #[serde(default)]
    pub signature_html: Option<String>,
    #[serde(default)]
    pub signature_text: Option<String>,
    /// The user's own Google client ID (section 5); Google sign-in only. On update, `None`
    /// keeps the saved one.
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    /// Its client secret (SPEC_GAPS G70): keychain only, never echoed back. On update,
    /// `None` keeps the saved one.
    #[serde(default, deserialize_with = "secret")]
    pub oauth_client_secret: Option<Secret>,
}

fn secret<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Secret>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.map(Secret::new))
}

/// An account as the UI sees it. No secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSummary {
    pub id: AccountId,
    pub name: String,
    pub email: String,
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub username: String,
    pub auth_kind: AuthKind,
    /// The Google client ID (not a secret); empty for password accounts.
    pub oauth_client_id: String,
    pub pinned_cert_sha256: Option<String>,
    pub enabled: bool,
    pub sync_days: u32,
    pub signature_html: String,
    pub signature_text: String,
}

/// One server-settings candidate (`account.resolve`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub username: String,
    pub source: ConfigSource,
    /// Login methods to offer, read from the host and its pre-login CAPABILITY.
    pub auth_methods: Vec<AuthKind>,
    /// Why the IMAP server could not be probed, for example `certificate` with the
    /// fingerprint to pin. `None` when the probe worked.
    pub probe_error: Option<AppError>,
}

/// The two halves of Test connection. `None` means that half passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestReport {
    pub imap_error: Option<AppError>,
    pub smtp_error: Option<AppError>,
    /// Folders `LIST` returned.
    pub folders: usize,
}

impl TestReport {
    pub fn passed(&self) -> bool {
        self.imap_error.is_none() && self.smtp_error.is_none()
    }

    fn first_error(&self) -> Option<AppError> {
        self.imap_error.clone().or_else(|| self.smtp_error.clone())
    }
}

/// The account service. Generic over the network so tests run on the fakes.
#[derive(Debug)]
pub struct Accounts<I, S, H> {
    store: Store,
    secrets: Arc<dyn SecretStore>,
    imap: I,
    smtp: S,
    http: H,
}

impl<I, S, H> Accounts<I, S, H>
where
    I: ImapConnector,
    S: SmtpCheck,
    H: HttpGet,
{
    pub fn new(store: Store, secrets: Arc<dyn SecretStore>, imap: I, smtp: S, http: H) -> Self {
        Self {
            store,
            secrets,
            imap,
            smtp,
            http,
        }
    }

    /// `account.resolve`: server-settings candidates and the login methods each offers.
    /// Empty means the UI shows the manual form.
    pub async fn resolve(&self, email: &str) -> Result<Vec<Candidate>, AppError> {
        let online = self
            .store
            .get_setting(ONLINE_LOOKUPS)
            .await?
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let found = mail_auth::resolve(email, online, &self.http).await?;
        let mut out = Vec::with_capacity(found.len());
        for s in found {
            let imap = endpoint_from_auth(&s.imap);
            let probe = self.imap.probe(&imap_config(&imap, None)).await;
            let by_host = mail_auth::auth_methods(&s.imap.host);
            let (auth_methods, probe_error) = match probe {
                Ok(caps) => (offered(&by_host, &caps), None),
                Err(err) => (
                    by_host.iter().map(|m| auth_kind(*m)).collect(),
                    Some(err.into()),
                ),
            };
            out.push(Candidate {
                imap,
                smtp: endpoint_from_auth(&s.smtp),
                username: s.username,
                source: match s.source {
                    mail_auth::Source::BuiltIn => ConfigSource::BuiltIn,
                    mail_auth::Source::Autoconfig => ConfigSource::Autoconfig,
                    mail_auth::Source::IspDb => ConfigSource::IspDb,
                },
                auth_methods,
                probe_error,
            });
        }
        Ok(out)
    }

    /// `account.test`: IMAP login plus `LIST` and SMTP `EHLO` plus `AUTH`, concurrently. No
    /// message is sent and nothing is saved.
    pub async fn test(&self, config: &AccountConfig) -> Result<TestReport, AppError> {
        let valid = validate(config)?;
        if config.auth_kind == AuthKind::OauthGoogle {
            return Err(AppError::invalid(
                "a Google account is tested when it signs in",
            ));
        }
        let password = config
            .password
            .clone()
            .ok_or_else(|| AppError::invalid("a password is required"))?;
        Ok(self.run_test(config, &valid, &password).await)
    }

    /// `account.add`. Saves the account, its default identity and its password, or for
    /// Google sign-in its client secret (G70, G71).
    pub async fn add(&self, config: AccountConfig) -> Result<AccountSummary, AppError> {
        let valid = validate(&config)?;
        let (secret, oauth) = match config.auth_kind {
            AuthKind::Password => {
                let password = config
                    .password
                    .clone()
                    .ok_or_else(|| AppError::invalid("a password is required"))?;
                if !config.save_anyway {
                    let report = self.run_test(&config, &valid, &password).await;
                    if let Some(err) = report.first_error() {
                        return Err(err);
                    }
                }
                (password, None)
            }
            AuthKind::OauthGoogle => {
                let client_id = config
                    .oauth_client_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| AppError::invalid("the Google client ID is required"))?;
                let secret = client_secret(config.oauth_client_secret.as_ref())
                    .ok_or_else(|| AppError::invalid("the Google client secret is required"))?;
                (secret, Some(client_id.to_string()))
            }
        };
        let account = NewAccount {
            name: valid.name.clone(),
            email: valid.email.clone(),
            imap_host: config.imap.host.trim().to_string(),
            imap_port: config.imap.port,
            imap_security: store_security(config.imap.security),
            smtp_host: config.smtp.host.trim().to_string(),
            smtp_port: config.smtp.port,
            smtp_security: store_security(config.smtp.security),
            username: config.username.trim().to_string(),
            auth_kind: match oauth {
                None => mail_store::AuthKind::Password,
                Some(_) => mail_store::AuthKind::OauthGoogle,
            },
            oauth_client_id: oauth.clone().unwrap_or_default(),
            pinned_cert_sha256: valid.pin_hex.clone(),
            sync_days: DEFAULT_SYNC_DAYS,
            save_sent_copy: SaveSentCopy::Auto,
            signature_html: config.signature_html.clone().unwrap_or_default(),
            signature_text: config.signature_text.clone().unwrap_or_default(),
            enabled: true,
            created_at: now(),
        };
        let identity = NewIdentity {
            name: valid.name.clone(),
            address: valid.email.clone(),
            reply_to: None,
            is_default: true,
        };
        let (id, _) = self.store.add_account(account, identity).await?;
        let saved = match oauth {
            None => self.save_password(id, secret).await,
            Some(_) => self.save_client_secret(id, secret).await,
        };
        if let Err(err) = saved {
            // No account without its secret: undo the insert.
            let _ = self.store.delete_account(id).await;
            return Err(err);
        }
        self.summary(id).await
    }

    /// `account.update`. A `None` password keeps the saved one; the connection test uses
    /// whichever password will be in effect. A Google account is saved without a test: its
    /// next sign-in tests it (G71).
    pub async fn update(
        &self,
        id: AccountId,
        config: AccountConfig,
    ) -> Result<AccountSummary, AppError> {
        let valid = validate(&config).map_err(|e| e.for_account(id))?;
        let existing = self.store.get_account(id).await?.config;
        if auth_kind_of(existing.auth_kind) != config.auth_kind {
            return Err(AppError::invalid("the sign-in method cannot change").for_account(id));
        }
        if config.auth_kind == AuthKind::OauthGoogle {
            return self.update_oauth(id, config, valid, existing).await;
        }
        let password = match &config.password {
            Some(p) => p.clone(),
            None => self.saved_password(id, &existing.username).await?,
        };
        if !config.save_anyway {
            let report = self.run_test(&config, &valid, &password).await;
            if let Some(err) = report.first_error() {
                return Err(err.for_account(id));
            }
        }
        let account = NewAccount {
            name: valid.name.clone(),
            email: valid.email.clone(),
            imap_host: config.imap.host.trim().to_string(),
            imap_port: config.imap.port,
            imap_security: store_security(config.imap.security),
            smtp_host: config.smtp.host.trim().to_string(),
            smtp_port: config.smtp.port,
            smtp_security: store_security(config.smtp.security),
            username: config.username.trim().to_string(),
            pinned_cert_sha256: valid.pin_hex.clone(),
            signature_html: config
                .signature_html
                .clone()
                .unwrap_or(existing.signature_html.clone()),
            signature_text: config
                .signature_text
                .clone()
                .unwrap_or(existing.signature_text.clone()),
            ..existing
        };
        self.store.update_account(id, account).await?;
        if let Some(p) = config.password {
            self.save_password(id, p).await?;
        }
        self.summary(id).await
    }

    /// `account.remove`: deletes the account's rows and keychain entries, never anything on
    /// the server (section 5). The UI asks for confirmation first.
    pub async fn remove(&self, id: AccountId) -> Result<(), AppError> {
        self.store.delete_account(id).await?;
        let secrets = Arc::clone(&self.secrets);
        blocking(move || secrets.delete_account(id).map_err(AppError::from)).await
    }

    /// `account.set_enabled`.
    pub async fn set_enabled(&self, id: AccountId, enabled: bool) -> Result<(), AppError> {
        Ok(self.store.set_account_enabled(id, enabled).await?)
    }

    /// Every account, oldest first.
    pub async fn list(&self) -> Result<Vec<AccountSummary>, AppError> {
        Ok(self
            .store
            .list_accounts()
            .await?
            .into_iter()
            .map(|a| summary(a.id, &a.config))
            .collect())
    }

    /// One IMAP login plus `LIST` with the saved credentials; returns the folder count. A
    /// rejected login is reported once and never retried (T09).
    pub async fn check_login(&self, id: AccountId) -> Result<usize, AppError> {
        let account = self.store.get_account(id).await?.config;
        let password = self
            .saved_password(id, &account.username)
            .await
            .map_err(|e| e.for_account(id))?;
        let imap = Endpoint {
            host: account.imap_host.clone(),
            port: account.imap_port,
            security: core_security(account.imap_security),
        };
        let pin = account
            .pinned_cert_sha256
            .as_deref()
            .and_then(mail_imap::parse_fingerprint);
        let creds = Credentials::Password {
            username: account.username.clone(),
            password: mail_imap::Secret::new(password.expose()),
        };
        let mut session = self
            .imap
            .connect(&imap_config(&imap, pin), &creds)
            .await
            .map_err(|e| AppError::from(e).for_account(id))?;
        let folders = session
            .list()
            .await
            .map_err(|e| AppError::from(e).for_account(id))?;
        let _ = session.logout().await;
        Ok(folders.len())
    }

    async fn update_oauth(
        &self,
        id: AccountId,
        config: AccountConfig,
        valid: Valid,
        existing: NewAccount,
    ) -> Result<AccountSummary, AppError> {
        let secret = client_secret(config.oauth_client_secret.as_ref());
        let client_id = match config.oauth_client_id.as_deref().map(str::trim) {
            None => existing.oauth_client_id.clone(),
            Some("") => {
                return Err(AppError::invalid("the Google client ID is required").for_account(id))
            }
            Some(c) => c.to_string(),
        };
        let account = NewAccount {
            name: valid.name.clone(),
            email: valid.email.clone(),
            imap_host: config.imap.host.trim().to_string(),
            imap_port: config.imap.port,
            imap_security: store_security(config.imap.security),
            smtp_host: config.smtp.host.trim().to_string(),
            smtp_port: config.smtp.port,
            smtp_security: store_security(config.smtp.security),
            username: config.username.trim().to_string(),
            oauth_client_id: client_id,
            pinned_cert_sha256: valid.pin_hex.clone(),
            signature_html: config
                .signature_html
                .clone()
                .unwrap_or(existing.signature_html.clone()),
            signature_text: config
                .signature_text
                .clone()
                .unwrap_or(existing.signature_text.clone()),
            ..existing
        };
        self.store.update_account(id, account).await?;
        if let Some(s) = secret {
            self.save_client_secret(id, s).await?;
        }
        self.summary(id).await
    }

    /// Test connection with a saved account's settings and the given login: after a
    /// Google sign-in, with the fresh access token (G71).
    pub async fn test_saved(
        &self,
        id: AccountId,
        login: &mail_auth::Login,
    ) -> Result<TestReport, AppError> {
        let a = self.store.get_account(id).await?.config;
        let imap = Endpoint {
            host: a.imap_host.clone(),
            port: a.imap_port,
            security: core_security(a.imap_security),
        };
        let smtp = Endpoint {
            host: a.smtp_host.clone(),
            port: a.smtp_port,
            security: core_security(a.smtp_security),
        };
        let pin = a
            .pinned_cert_sha256
            .as_deref()
            .and_then(mail_imap::parse_fingerprint);
        let secret = login.secret.expose().to_string();
        let (creds, smtp_login) = match login.method {
            AuthMethod::Password => (
                Credentials::Password {
                    username: login.username.clone(),
                    password: mail_imap::Secret::new(&secret),
                },
                SmtpLogin::Password {
                    username: login.username.clone(),
                    password: secret,
                },
            ),
            AuthMethod::OauthGoogle => (
                Credentials::XOAuth2 {
                    username: login.username.clone(),
                    access_token: mail_imap::Secret::new(&secret),
                },
                SmtpLogin::XOAuth2 {
                    username: login.username.clone(),
                    token: secret,
                },
            ),
        };
        Ok(self
            .probe_both(
                &imap_config(&imap, pin),
                &smtp_config(&smtp, pin),
                &creds,
                &smtp_login,
            )
            .await)
    }

    async fn summary(&self, id: AccountId) -> Result<AccountSummary, AppError> {
        let account = self.store.get_account(id).await?;
        Ok(summary(account.id, &account.config))
    }

    async fn save_password(&self, id: AccountId, password: Secret) -> Result<(), AppError> {
        let secrets = Arc::clone(&self.secrets);
        blocking(move || {
            mail_auth::store_password(secrets.as_ref(), id, &password).map_err(AppError::from)
        })
        .await
        .map_err(|e| e.for_account(id))
    }

    async fn save_client_secret(&self, id: AccountId, secret: Secret) -> Result<(), AppError> {
        let secrets = Arc::clone(&self.secrets);
        blocking(move || {
            mail_auth::store_client_secret(secrets.as_ref(), id, &secret).map_err(AppError::from)
        })
        .await
        .map_err(|e| e.for_account(id))
    }

    async fn saved_password(&self, id: AccountId, username: &str) -> Result<Secret, AppError> {
        let secrets = Arc::clone(&self.secrets);
        let username = username.to_string();
        blocking(move || {
            mail_auth::password_login(secrets.as_ref(), id, &username)
                .map(|login| login.secret)
                .map_err(AppError::from)
        })
        .await
    }

    async fn run_test(
        &self,
        config: &AccountConfig,
        valid: &Valid,
        password: &Secret,
    ) -> TestReport {
        let username = config.username.trim();
        let creds = Credentials::Password {
            username: username.to_string(),
            password: mail_imap::Secret::new(password.expose()),
        };
        let smtp_login = SmtpLogin::Password {
            username: username.to_string(),
            password: password.expose().to_string(),
        };
        self.probe_both(
            &imap_config(&config.imap, valid.pin),
            &smtp_config(&config.smtp, valid.pin),
            &creds,
            &smtp_login,
        )
        .await
    }

    /// Both halves of Test connection, concurrently.
    async fn probe_both(
        &self,
        imap_cfg: &ConnectConfig,
        smtp_cfg: &SmtpConfig,
        creds: &Credentials,
        smtp_login: &SmtpLogin,
    ) -> TestReport {
        let imap = async {
            let mut session = self.imap.connect(imap_cfg, creds).await?;
            let folders = session.list().await?.len();
            let _ = session.logout().await;
            Ok::<usize, mail_imap::ImapError>(folders)
        };
        let smtp = self.smtp.check_login(smtp_cfg, smtp_login);
        let (imap, smtp) = tokio::join!(imap, smtp);
        let (folders, imap_error) = match imap {
            Ok(n) => (n, None),
            Err(e) => (0, Some(AppError::from(e))),
        };
        TestReport {
            imap_error,
            smtp_error: smtp.err().map(AppError::from),
            folders,
        }
    }
}

/// A validated form: the parts that need normalizing.
struct Valid {
    name: String,
    email: String,
    pin: Option<[u8; 32]>,
    pin_hex: Option<String>,
}

/// A client secret the form sent, trimmed; an empty one is no secret.
fn client_secret(secret: Option<&Secret>) -> Option<Secret> {
    secret
        .map(|s| s.expose().trim())
        .filter(|s| !s.is_empty())
        .map(Secret::new)
}

fn auth_kind_of(k: mail_store::AuthKind) -> AuthKind {
    match k {
        mail_store::AuthKind::Password => AuthKind::Password,
        mail_store::AuthKind::OauthGoogle => AuthKind::OauthGoogle,
    }
}

fn validate(config: &AccountConfig) -> Result<Valid, AppError> {
    mail_auth::split_address(&config.email)?;
    for (what, ep) in [("IMAP", &config.imap), ("SMTP", &config.smtp)] {
        let host = ep.host.trim();
        if host.is_empty() || host.contains(char::is_whitespace) || ep.port == 0 {
            return Err(AppError::invalid(format!("the {what} server is not valid")));
        }
    }
    if config.username.trim().is_empty() {
        return Err(AppError::invalid("a username is required"));
    }
    let pin = match config.pinned_cert_sha256.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) => Some(
            mail_imap::parse_fingerprint(text)
                .ok_or_else(|| AppError::invalid("the certificate fingerprint is not valid"))?,
        ),
    };
    let email = config.email.trim().to_string();
    let name = match config.name.trim() {
        "" => email.clone(),
        n => n.to_string(),
    };
    Ok(Valid {
        name,
        email,
        pin,
        pin_hex: pin.map(|p| mail_imap::hex(&p)),
    })
}

/// The login methods to offer: the host's, filtered by what the server advertises.
fn offered(by_host: &[AuthMethod], caps: &Capabilities) -> Vec<AuthKind> {
    by_host
        .iter()
        .filter(|m| match m {
            AuthMethod::Password => caps.auth_plain || !caps.login_disabled,
            AuthMethod::OauthGoogle => caps.auth_xoauth2,
        })
        .map(|m| auth_kind(*m))
        .collect()
}

fn auth_kind(m: AuthMethod) -> AuthKind {
    match m {
        AuthMethod::Password => AuthKind::Password,
        AuthMethod::OauthGoogle => AuthKind::OauthGoogle,
    }
}

fn imap_config(ep: &Endpoint, pin: Option<[u8; 32]>) -> ConnectConfig {
    ConnectConfig {
        host: ep.host.trim().to_string(),
        port: ep.port,
        security: match ep.security {
            Security::Tls => mail_imap::Security::Tls,
            Security::Starttls => mail_imap::Security::StartTls,
        },
        pinned_cert_sha256: pin,
        connect_timeout: mail_imap::CONNECT_TIMEOUT,
    }
}

fn smtp_config(ep: &Endpoint, pin: Option<[u8; 32]>) -> SmtpConfig {
    SmtpConfig {
        host: ep.host.trim().to_string(),
        port: ep.port,
        security: match ep.security {
            Security::Tls => mail_smtp::Security::Tls,
            Security::Starttls => mail_smtp::Security::StartTls,
        },
        pinned_cert_sha256: pin,
        connect_timeout: mail_imap::CONNECT_TIMEOUT,
    }
}

fn endpoint_from_auth(ep: &mail_auth::Endpoint) -> Endpoint {
    Endpoint {
        host: ep.host.clone(),
        port: ep.port,
        security: match ep.security {
            mail_auth::Security::Tls => Security::Tls,
            mail_auth::Security::StartTls => Security::Starttls,
        },
    }
}

fn store_security(s: Security) -> mail_store::Security {
    match s {
        Security::Tls => mail_store::Security::Tls,
        Security::Starttls => mail_store::Security::StartTls,
    }
}

fn core_security(s: mail_store::Security) -> Security {
    match s {
        mail_store::Security::Tls => Security::Tls,
        mail_store::Security::StartTls => Security::Starttls,
    }
}

fn summary(id: AccountId, a: &NewAccount) -> AccountSummary {
    AccountSummary {
        id,
        name: a.name.clone(),
        email: a.email.clone(),
        imap: Endpoint {
            host: a.imap_host.clone(),
            port: a.imap_port,
            security: core_security(a.imap_security),
        },
        smtp: Endpoint {
            host: a.smtp_host.clone(),
            port: a.smtp_port,
            security: core_security(a.smtp_security),
        },
        username: a.username.clone(),
        auth_kind: auth_kind_of(a.auth_kind),
        oauth_client_id: a.oauth_client_id.clone(),
        pinned_cert_sha256: a.pinned_cert_sha256.clone(),
        enabled: a.enabled,
        sync_days: a.sync_days,
        signature_html: a.signature_html.clone(),
        signature_text: a.signature_text.clone(),
    }
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Runs blocking keychain work off the async threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| AppError::new(ErrorCode::Internal, "background task failed"))?
}
