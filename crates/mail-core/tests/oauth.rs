//! M8 on the fakes: Google sign-in end to end (section 5; SPEC_GAPS G70, G71). The browser
//! is played by a plain HTTP GET on the loopback redirect, and Google's token endpoint by
//! the scripted `FakeHttp`. T31 and T32 themselves are manual (docs/MANUAL_TESTS.md); these
//! are their automated counterparts.
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{config, FakeHttp, TempDir};
use mail_core::compose::{DraftContent, Timing};
use mail_core::views::{AddressView, ListRequest, Role, Scope};
use mail_core::{
    AccountConfig, AuthKind, Core, CoreConfig, CoreEvent, ErrorCode, EventSink, StateName,
};
use mail_imap::fake::{FakeMessage, FakeServer, Op};
use mail_imap::SpecialUse;
use mail_secrets::{MemoryStore, Secret, SecretStore};
use mail_smtp::fake::FakeSmtp;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const USER: &str = "deep@workspace.test";
const BOB: &str = "bob@test.local";
const BOB_PW: &str = "bob-pw";
const CLIENT_ID: &str = "1234-abc.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "GOCSPX-client-secret";
const ACCESS_1: &str = "ya29.access-token-one";
const ACCESS_2: &str = "ya29.access-token-two";
const REFRESH_1: &str = "1//refresh-token-one";
const REFRESH_2: &str = "1//refresh-token-two";

type TestCore = Core<FakeServer, FakeSmtp, FakeHttp>;

struct Setup {
    imap: FakeServer,
    smtp: FakeSmtp,
    http: FakeHttp,
    secrets: Arc<MemoryStore>,
    events: Arc<Mutex<Vec<CoreEvent>>>,
    core: Arc<TestCore>,
}

fn servers() -> (FakeServer, FakeSmtp) {
    let imap = FakeServer::new(&[
        "IMAP4rev1",
        "IDLE",
        "MOVE",
        "UIDPLUS",
        "SPECIAL-USE",
        "AUTH=PLAIN",
        "AUTH=XOAUTH2",
    ]);
    imap.add_folder("Sent", Some(SpecialUse::Sent));
    imap.add_folder("Drafts", Some(SpecialUse::Drafts));
    imap.add_user(BOB, BOB_PW);
    let smtp = FakeSmtp::default();
    smtp.add_user(BOB, BOB_PW);
    (imap, smtp)
}

async fn start(
    dir: &TempDir,
    imap: &FakeServer,
    smtp: &FakeSmtp,
    http: &FakeHttp,
    secrets: &Arc<MemoryStore>,
) -> Setup {
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path(),
            secrets: Arc::clone(secrets) as Arc<dyn SecretStore>,
            imap: imap.clone(),
            smtp: smtp.clone(),
            http: http.clone(),
        },
        sink,
    )
    .await
    .unwrap();
    core.set_timing(Timing {
        undo_window: Duration::ZERO,
        draft_upload: Duration::from_secs(3600),
        outbox_tick: Duration::from_millis(50),
    });
    Setup {
        imap: imap.clone(),
        smtp: smtp.clone(),
        http: http.clone(),
        secrets: Arc::clone(secrets),
        events,
        core,
    }
}

fn google_config() -> AccountConfig {
    let mut c = config(USER, USER, "");
    c.imap.host = "imap.gmail.com".into();
    c.smtp.host = "smtp.gmail.com".into();
    c.auth_kind = AuthKind::OauthGoogle;
    c.password = None;
    c.oauth_client_id = Some(format!(" {CLIENT_ID} "));
    c.oauth_client_secret = Some(Secret::new(CLIENT_SECRET));
    c
}

fn token_json(access: &str, lifetime: u32, refresh: Option<&str>) -> String {
    match refresh {
        Some(r) => format!(
            r#"{{"access_token":"{access}","expires_in":{lifetime},"refresh_token":"{r}","token_type":"Bearer","scope":"https://mail.google.com/"}}"#
        ),
        None => format!(
            r#"{{"access_token":"{access}","expires_in":{lifetime},"token_type":"Bearer"}}"#
        ),
    }
}

async fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One query parameter of a URL, percent-decoded.
fn param(url: &str, name: &str) -> String {
    let query = url.split_once('?').unwrap().1;
    let raw = query
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
        .unwrap();
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(&raw[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// Plays the browser after consent: Google redirects to the loopback address.
async fn browser_redirect(consent_url: &str, query: &str) -> String {
    let redirect = param(consent_url, "redirect_uri");
    let state = param(consent_url, "state");
    let addr = redirect
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let request = format!("GET /?state={state}&{query} HTTP/1.1\r\nHost: {addr}\r\n\r\n");
    s.write_all(request.as_bytes()).await.unwrap();
    let mut page = String::new();
    s.read_to_string(&mut page).await.unwrap();
    page
}

impl Setup {
    fn latest_state(&self, account: i64) -> Option<(StateName, Option<ErrorCode>)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|e| match e {
                CoreEvent::AccountState {
                    account_id,
                    state,
                    last_error,
                } if *account_id == account => Some((*state, last_error.as_ref().map(|e| e.code))),
                _ => None,
            })
    }

    async fn wait_state(&self, account: i64, want: StateName) {
        wait_for(&format!("{want:?}"), || {
            self.latest_state(account).map(|s| s.0) == Some(want)
        })
        .await;
    }

    fn oauth_done(&self) -> Vec<(i64, bool, Option<ErrorCode>)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                CoreEvent::OauthDone {
                    account_id,
                    ok,
                    error,
                } => Some((*account_id, *ok, error.as_ref().map(|e| e.code))),
                _ => None,
            })
            .collect()
    }

    async fn wait_oauth_done(&self, n: usize) -> (i64, bool, Option<ErrorCode>) {
        wait_for("oauth-done", || self.oauth_done().len() >= n).await;
        self.oauth_done()[n - 1]
    }

    /// Adds the Google account and signs it in with `access`/`refresh` from the token
    /// endpoint; the fake servers accept that access token.
    async fn add_and_sign_in(&self, access: &str, refresh: &str) -> i64 {
        let summary = self.core.account_add(google_config()).await.unwrap();
        self.sign_in(summary.id, access, refresh, 1).await;
        summary.id
    }

    async fn sign_in(&self, account: i64, access: &str, refresh: &str, nth: usize) {
        self.imap.set_password(USER, access);
        self.smtp.add_user(USER, access);
        self.http
            .token_reply(200, &token_json(access, 3599, Some(refresh)));
        let url = self.core.account_oauth_start(account).await.unwrap();
        let page = browser_redirect(
            &url,
            "code=4%2F0Acode&scope=https%3A%2F%2Fmail.google.com%2F",
        )
        .await;
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert_eq!(self.wait_oauth_done(nth).await, (account, true, None));
        self.wait_state(account, StateName::Idle).await;
    }

    fn connect_mechanisms(&self) -> Vec<String> {
        self.imap
            .calls()
            .iter()
            .filter(|c| c.op == Op::Connect)
            .map(|c| c.arg.clone())
            .collect()
    }

    async fn inbox_subjects(&self, account: i64) -> Vec<String> {
        let folders = self.core.folder_list(account).await.unwrap();
        let Some(inbox) = folders.iter().find(|f| f.role == Some(Role::Inbox)) else {
            return Vec::new();
        };
        self.core
            .list_page(ListRequest {
                scope: Scope::Folder {
                    folder_id: inbox.id,
                },
                cursor: None,
                limit: 50,
                group_by_thread: false,
            })
            .await
            .unwrap()
            .rows
            .into_iter()
            .map(|r| r.subject)
            .collect()
    }

    async fn outbox_state(&self, account: i64, id: i64) -> Option<&'static str> {
        let list = self.core.outbox_list(account).await.unwrap();
        list.into_iter().find(|o| o.id == id).map(|o| o.state)
    }

    /// A saved draft to bob, and its identity.
    async fn draft(&self, account: i64, subject: &str) -> (i64, i64) {
        let view = self.core.compose_new(account, None).await.unwrap();
        let identity = view.identities[0].id.unwrap();
        let content = DraftContent {
            to: vec![AddressView {
                name: String::new(),
                addr: BOB.into(),
            }],
            subject: subject.into(),
            html: "<p>Hi</p>".into(),
            text: "Hi".into(),
            ..view.content
        };
        self.core.draft_save(view.draft_id, content).await.unwrap();
        (view.draft_id, identity)
    }
}

/// Whether any file of the data folder holds `needle`. On Windows a file SQLite still has
/// open is locked; the read is retried until the last connection has closed.
async fn file_contains(dir: &TempDir, needle: &str) -> bool {
    let start = Instant::now();
    loop {
        let mut found = false;
        let mut locked = false;
        for e in std::fs::read_dir(dir.path()).unwrap() {
            let path = e.unwrap().path();
            if !path.is_file() {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => found |= bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                Err(_) => locked = true,
            }
        }
        if !locked {
            return found;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "data folder still locked"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn message(subject: &str) -> FakeMessage {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    FakeMessage::new(
        format!("From: Bob <{BOB}>\r\nTo: {USER}\r\nSubject: {subject}\r\nMessage-ID: <{subject}@test>\r\n\r\nBody\r\n"),
        now,
    )
}

/// T31, first half: saved before sign-in (G71) without connecting, signed in through the
/// browser, synced and sent over XOAUTH2; the access token never reaches the keychain or the
/// database.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t31_save_sign_in_sync_and_send_over_xoauth2() {
    let dir = TempDir::new("oauth");
    let (imap, smtp) = servers();
    imap.add_user(USER, "nothing-yet");
    imap.deliver("INBOX", message("welcome")).unwrap();
    let s = start(
        &dir,
        &imap,
        &smtp,
        &FakeHttp::default(),
        &Arc::new(MemoryStore::default()),
    )
    .await;

    let summary = s.core.account_add(google_config()).await.unwrap();
    assert_eq!(summary.auth_kind, AuthKind::OauthGoogle);
    assert_eq!(summary.oauth_client_id, CLIENT_ID);
    // Not signed in yet: the actor waits for the user and makes no connection.
    s.wait_state(summary.id, StateName::NeedsAttention).await;
    assert_eq!(
        s.latest_state(summary.id).unwrap().1,
        Some(ErrorCode::AuthFailed)
    );
    assert!(s.connect_mechanisms().is_empty());
    assert!(s.http.posts().is_empty());
    assert_eq!(
        s.secrets.keys(),
        vec![format!("account:{}:oauth_client_secret", summary.id)]
    );

    s.sign_in(summary.id, ACCESS_1, REFRESH_1, 1).await;
    let subjects = {
        let start = Instant::now();
        loop {
            let found = s.inbox_subjects(summary.id).await;
            if !found.is_empty() || start.elapsed() > Duration::from_secs(20) {
                break found;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    assert_eq!(subjects, vec!["welcome".to_string()]);
    // Test connection and the actor logged in with XOAUTH2 only.
    let mechanisms = s.connect_mechanisms();
    assert!(
        !mechanisms.is_empty() && mechanisms.iter().all(|m| m == "xoauth2"),
        "{mechanisms:?}"
    );
    assert_eq!(
        s.smtp.mechanisms(),
        vec!["xoauth2"],
        "the SMTP half of the test"
    );

    // The code exchange: one POST to Google's token endpoint with the PKCE verifier and the
    // client secret, which went nowhere else.
    let posts = s.http.posts();
    assert_eq!(posts.len(), 1, "the cached token serves every login");
    assert_eq!(posts[0].0, mail_auth::oauth::TOKEN_URL);
    assert!(posts[0].1.contains("grant_type=authorization_code"));
    assert!(posts[0].1.contains("code_verifier="));
    assert!(posts[0].1.contains("client_secret=GOCSPX-client-secret"));

    // Send over XOAUTH2 with the token in memory.
    let (draft, identity) = s.draft(summary.id, "Over OAuth").await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    wait_for("delivered", || s.smtp.deliveries().len() == 1).await;
    assert_eq!(s.smtp.mechanisms(), vec!["xoauth2", "xoauth2"]);
    let _ = id;

    // Secrets: the client secret and the refresh token in the keychain, the access token
    // nowhere at rest.
    let k = |kind: &str| format!("account:{}:{kind}", summary.id);
    assert_eq!(
        s.secrets.keys(),
        vec![k("oauth_client_secret"), k("refresh_token")]
    );
    assert_eq!(
        s.secrets.get(&k("refresh_token")).unwrap(),
        Some(Secret::new(REFRESH_1))
    );
    for key in s.secrets.keys() {
        assert_ne!(s.secrets.get(&key).unwrap(), Some(Secret::new(ACCESS_1)));
    }
    s.core.shutdown().await;
    drop(s);
    for secret in [ACCESS_1, REFRESH_1, CLIENT_SECRET] {
        assert!(
            !file_contains(&dir, secret).await,
            "{secret} reached the data folder"
        );
    }
}

/// T31, second half: access tokens live in memory only, so a restart refreshes with the
/// refresh token from the keychain, with no user action; the new token serves IMAP and SMTP.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t31_tokens_refresh_without_user_action() {
    let dir = TempDir::new("oauth");
    let (imap, smtp) = servers();
    let http = FakeHttp::default();
    let secrets = Arc::new(MemoryStore::default());
    let s = start(&dir, &imap, &smtp, &http, &secrets).await;
    let account = s.add_and_sign_in(ACCESS_1, REFRESH_1).await;
    s.core.shutdown().await;

    // The old token stops working, as after its hour; Google hands out a new one.
    imap.set_password(USER, ACCESS_2);
    smtp.add_user(USER, ACCESS_2);
    http.token_reply(200, &token_json(ACCESS_2, 3599, None));
    let s = start(&dir, &imap, &smtp, &http, &secrets).await;
    s.wait_state(account, StateName::Idle).await;
    let posts = http.posts();
    assert_eq!(posts.len(), 2);
    assert!(posts[1].1.contains("grant_type=refresh_token"));
    assert!(posts[1]
        .1
        .contains("refresh_token=1%2F%2Frefresh-token-one"));
    // Google kept the refresh token; the keychain still has it.
    assert_eq!(
        secrets
            .get(&format!("account:{account}:refresh_token"))
            .unwrap(),
        Some(Secret::new(REFRESH_1))
    );
    let (draft, identity) = s.draft(account, "After refresh").await;
    s.core.draft_send(draft, identity, None).await.unwrap();
    wait_for("delivered", || smtp.deliveries().len() == 1).await;
    assert_eq!(http.posts().len(), 2, "the send used the refreshed token");
    assert!(s.oauth_done().is_empty(), "no sign-in was needed");
    s.core.shutdown().await;
}

/// T32: the grant is revoked. The account goes to NeedsAttention after one refused refresh
/// and never retries; another account keeps syncing; signing in again restores the account
/// with its mail, drafts and outbox intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t32_revoked_needs_attention_and_sign_in_again_loses_nothing() {
    let dir = TempDir::new("oauth");
    let (imap, smtp) = servers();
    let http = FakeHttp::default();
    let secrets = Arc::new(MemoryStore::default());
    imap.deliver("INBOX", message("kept")).unwrap();
    let s = start(&dir, &imap, &smtp, &http, &secrets).await;
    let account = s.add_and_sign_in(ACCESS_1, REFRESH_1).await;
    let bob = s
        .core
        .account_add(config(BOB, BOB, BOB_PW))
        .await
        .unwrap()
        .id;
    s.wait_state(bob, StateName::Idle).await;
    let (_, identity) = s.draft(account, "Unsent draft").await;
    let (scheduled, _) = s.draft(account, "Scheduled").await;
    let later = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 3600;
    let outbox_id = s
        .core
        .draft_send(scheduled, identity, Some(later))
        .await
        .unwrap();
    let start_wait = Instant::now();
    while s.inbox_subjects(account).await.is_empty() {
        assert!(start_wait.elapsed() < Duration::from_secs(20));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    s.core.shutdown().await;

    // Revoked at myaccount.google.com: the access token and the grant both stop working.
    imap.set_password(USER, "revoked");
    http.token_reply(
        400,
        r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
    );
    imap.clear_calls();
    let s = start(&dir, &imap, &smtp, &http, &secrets).await;
    s.wait_state(account, StateName::NeedsAttention).await;
    assert_eq!(
        s.latest_state(account).unwrap().1,
        Some(ErrorCode::AuthFailed)
    );
    s.wait_state(bob, StateName::Idle).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(http.posts().len(), 2, "one refused refresh, no retries");
    assert!(
        s.connect_mechanisms().iter().all(|m| m == "password"),
        "no IMAP login for the revoked account"
    );
    assert_eq!(
        s.latest_state(account).unwrap().0,
        StateName::NeedsAttention
    );
    assert_eq!(s.latest_state(bob).unwrap().0, StateName::Idle);

    // Sign in again (the banner's button): everything is still there.
    s.sign_in(account, "ya29.after-sign-in-again", REFRESH_2, 1)
        .await;
    assert_eq!(s.inbox_subjects(account).await, vec!["kept".to_string()]);
    let outbox = s.core.outbox_list(account).await.unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!((outbox[0].id, outbox[0].state), (outbox_id, "queued"));
    let store = dir.store();
    assert!(store.get_draft(scheduled).await.is_ok());
    assert_eq!(
        secrets
            .get(&format!("account:{account}:refresh_token"))
            .unwrap(),
        Some(Secret::new(REFRESH_2))
    );
    s.core.shutdown().await;
}

/// Google out of reach is network trouble: Backoff, not NeedsAttention, and a send waits in
/// `retry_wait` instead of failing. Both recover once the token endpoint answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_token_endpoint_is_transient() {
    let dir = TempDir::new("oauth");
    let (imap, smtp) = servers();
    let http = FakeHttp::default();
    let secrets = Arc::new(MemoryStore::default());
    let s = start(&dir, &imap, &smtp, &http, &secrets).await;
    let account = s.add_and_sign_in(ACCESS_1, REFRESH_1).await;
    s.core.shutdown().await;

    // Nothing scripted: the token endpoint is unreachable.
    let s = start(&dir, &imap, &smtp, &http, &secrets).await;
    s.wait_state(account, StateName::Backoff).await;
    assert_eq!(s.latest_state(account).unwrap().1, Some(ErrorCode::Network));
    let (draft, identity) = s.draft(account, "Offline Google").await;
    let id = s.core.draft_send(draft, identity, None).await.unwrap();
    let waited = Instant::now();
    while s.outbox_state(account, id).await != Some("retry_wait") {
        assert!(
            waited.elapsed() < Duration::from_secs(20),
            "never retry_wait"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    http.token_reply(200, &token_json(ACCESS_1, 3599, None));
    s.wait_state(account, StateName::Idle).await;
    s.core.shutdown().await;
}

/// A denied consent ends with `oauth-done` and code `oauth`; nothing is saved. A second
/// start replaces the first listener. Removing the account deletes its keychain entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_sign_in_reports_oauth_and_remove_cleans_up() {
    let dir = TempDir::new("oauth");
    let (imap, smtp) = servers();
    let http = FakeHttp::default();
    let secrets = Arc::new(MemoryStore::default());
    let s = start(&dir, &imap, &smtp, &http, &secrets).await;
    let account = s.core.account_add(google_config()).await.unwrap().id;

    let first = s.core.account_oauth_start(account).await.unwrap();
    let second = s.core.account_oauth_start(account).await.unwrap();
    assert_ne!(param(&first, "state"), param(&second, "state"));
    let page = browser_redirect(&second, "error=access_denied").await;
    assert!(page.contains("did not complete"));
    assert_eq!(
        s.wait_oauth_done(1).await,
        (account, false, Some(ErrorCode::Oauth))
    );
    assert!(http.posts().is_empty(), "no code, no exchange");
    assert_eq!(
        s.secrets.keys(),
        vec![format!("account:{account}:oauth_client_secret")]
    );
    // The first listener was closed when the second start replaced it.
    let first_addr = param(&first, "redirect_uri")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    assert!(tokio::net::TcpStream::connect(first_addr).await.is_err());

    // A password account cannot start a Google sign-in.
    let bob = s
        .core
        .account_add(config(BOB, BOB, BOB_PW))
        .await
        .unwrap()
        .id;
    assert_eq!(
        s.core.account_oauth_start(bob).await.unwrap_err().code,
        ErrorCode::InvalidInput
    );
    // Editing keeps the method and the saved client; it cannot switch to a password.
    let mut edit = google_config();
    edit.oauth_client_id = None;
    edit.oauth_client_secret = None;
    edit.name = "Work".into();
    let edited = s.core.account_update(account, edit).await.unwrap();
    assert_eq!(
        (edited.name.as_str(), edited.oauth_client_id.as_str()),
        ("Work", CLIENT_ID)
    );
    let mut switch = config(USER, USER, "pw");
    switch.imap.host = "imap.gmail.com".into();
    assert_eq!(
        s.core
            .account_update(account, switch)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );

    // Cancelling the new-account form removes the account and its keychain entries (G71).
    s.core.account_remove(account).await.unwrap();
    assert!(s
        .secrets
        .keys()
        .iter()
        .all(|k| !k.starts_with(&format!("account:{account}:"))));
    s.core.shutdown().await;
}
