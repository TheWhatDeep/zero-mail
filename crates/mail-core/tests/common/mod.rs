#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use mail_auth::{HttpFailure, HttpGet, HttpPost, HttpReply};
use mail_core::{AccountConfig, AuthKind, Endpoint, Security};
use mail_secrets::{MemoryStore, Secret};
use mail_store::Store;

/// A scratch directory under the OS temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zero-mail-core-{tag}-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn store(&self) -> Store {
        Store::open(&self.0.join("mail.db")).unwrap()
    }

    pub fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Serves fixed pages and records every URL asked for.
#[derive(Debug, Default, Clone)]
pub struct FakeHttp {
    pub pages: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    pub asked: Arc<Mutex<Vec<String>>>,
    pub token_replies: Arc<Mutex<VecDeque<Result<HttpReply, HttpFailure>>>>,
    pub posts: Arc<Mutex<Vec<(String, String)>>>,
}

impl HttpGet for FakeHttp {
    async fn get(&self, url: &str) -> Option<Vec<u8>> {
        self.asked.lock().unwrap().push(url.to_string());
        self.pages.lock().unwrap().get(url).cloned()
    }
}

/// A scripted token endpoint: answers in order, and is unreachable when nothing is left.
/// Every form posted is kept.
impl HttpPost for FakeHttp {
    async fn post_form(&self, url: &str, body: Vec<u8>) -> Result<HttpReply, HttpFailure> {
        self.posts
            .lock()
            .unwrap()
            .push((url.to_string(), String::from_utf8(body).unwrap()));
        self.token_replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(HttpFailure("unreachable".into())))
    }
}

impl FakeHttp {
    /// Queues one token endpoint answer.
    pub fn token_reply(&self, status: u16, json: &str) {
        self.token_replies.lock().unwrap().push_back(Ok(HttpReply {
            status,
            body: json.as_bytes().to_vec(),
        }));
    }

    /// The forms posted so far, as (url, body).
    pub fn posts(&self) -> Vec<(String, String)> {
        self.posts.lock().unwrap().clone()
    }
}

pub fn secrets() -> Arc<MemoryStore> {
    Arc::new(MemoryStore::default())
}

pub fn config(email: &str, username: &str, password: &str) -> AccountConfig {
    AccountConfig {
        name: "Test User".into(),
        email: email.into(),
        imap: Endpoint {
            host: "imap.test.local".into(),
            port: 993,
            security: Security::Tls,
        },
        smtp: Endpoint {
            host: "smtp.test.local".into(),
            port: 465,
            security: Security::Tls,
        },
        username: username.into(),
        auth_kind: AuthKind::Password,
        password: Some(Secret::new(password)),
        pinned_cert_sha256: None,
        save_anyway: false,
        signature_html: None,
        signature_text: None,
        oauth_client_id: None,
        oauth_client_secret: None,
    }
}
