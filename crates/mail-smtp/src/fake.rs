//! A scripted SMTP login check for tests (`fake` feature). Never used by the app.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::{SmtpCheck, SmtpConfig, SmtpError};

/// Accepts the users it knows; cheap to clone, clones share state.
#[derive(Debug, Clone, Default)]
pub struct FakeSmtp {
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    users: BTreeMap<String, String>,
    fail_next: Option<SmtpError>,
    attempts: usize,
}

impl FakeSmtp {
    pub fn add_user(&self, username: &str, password: &str) {
        self.lock()
            .users
            .insert(username.to_string(), password.to_string());
    }

    /// The next check fails with `error`.
    pub fn fail_next(&self, error: SmtpError) {
        self.lock().fail_next = Some(error);
    }

    /// How many logins were attempted.
    pub fn attempts(&self) -> usize {
        self.lock().attempts
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SmtpCheck for FakeSmtp {
    async fn check_login(
        &self,
        _config: &SmtpConfig,
        username: &str,
        password: &str,
    ) -> Result<(), SmtpError> {
        let mut s = self.lock();
        s.attempts += 1;
        if let Some(err) = s.fail_next.take() {
            return Err(err);
        }
        match s.users.get(username) {
            Some(p) if p == password => Ok(()),
            _ => Err(SmtpError::AuthFailed),
        }
    }
}
