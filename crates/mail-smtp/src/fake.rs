//! A scripted SMTP server for tests (`fake` feature): the login check and submission.
//! Never used by the app.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use crate::{Envelope, SmtpCheck, SmtpConfig, SmtpError, SmtpLogin, SmtpSubmit, SubmitError};

/// One accepted message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub envelope: Envelope,
    pub raw: Vec<u8>,
}

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
    deliveries: Vec<Delivery>,
    /// Recipients refused with 550.
    rejected: Vec<String>,
    /// Failures for the next submissions, in order.
    submit_failures: VecDeque<SubmitError>,
    /// The next submission never answers after DATA (a crash after DATA, T13).
    hang_after_data: bool,
    /// Submissions that reached DATA.
    data_started: usize,
    size_limit: Option<u64>,
    submits: usize,
    /// The mechanism of every login, check or submission: `password` or `xoauth2`.
    mechanisms: Vec<&'static str>,
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

    /// The mechanism of every login so far, in order.
    pub fn mechanisms(&self) -> Vec<&'static str> {
        self.lock().mechanisms.clone()
    }

    /// How many logins were attempted.
    pub fn attempts(&self) -> usize {
        self.lock().attempts
    }

    /// Every message accepted so far.
    pub fn deliveries(&self) -> Vec<Delivery> {
        self.lock().deliveries.clone()
    }

    /// How many submissions were attempted.
    pub fn submits(&self) -> usize {
        self.lock().submits
    }

    /// How many submissions reached DATA.
    pub fn data_started(&self) -> usize {
        self.lock().data_started
    }

    /// `RCPT TO` for this address answers 550.
    pub fn reject_recipient(&self, addr: &str) {
        self.lock().rejected.push(addr.to_ascii_lowercase());
    }

    /// The next submission fails with `error`. Failures queue in order.
    pub fn fail_next_submit(&self, error: SubmitError) {
        self.lock().submit_failures.push_back(error);
    }

    /// The next submission sends DATA and then never hears back.
    pub fn hang_after_data(&self) {
        self.lock().hang_after_data = true;
    }

    /// The `SIZE` the server announces.
    pub fn set_size_limit(&self, limit: Option<u64>) {
        self.lock().size_limit = limit;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn mechanism(login: &SmtpLogin) -> &'static str {
    match login {
        SmtpLogin::Password { .. } => "password",
        SmtpLogin::XOAuth2 { .. } => "xoauth2",
    }
}

impl SmtpCheck for FakeSmtp {
    async fn check_login(&self, _config: &SmtpConfig, login: &SmtpLogin) -> Result<(), SmtpError> {
        let mut s = self.lock();
        s.attempts += 1;
        s.mechanisms.push(mechanism(login));
        if let Some(err) = s.fail_next.take() {
            return Err(err);
        }
        let (username, password) = match login {
            SmtpLogin::Password { username, password } => (username, password),
            SmtpLogin::XOAuth2 { username, token } => (username, token),
        };
        match s.users.get(username) {
            Some(p) if p == password => Ok(()),
            _ => Err(SmtpError::AuthFailed),
        }
    }
}

impl SmtpSubmit for FakeSmtp {
    async fn submit(
        &self,
        _config: &SmtpConfig,
        login: &SmtpLogin,
        envelope: &Envelope,
        raw: &[u8],
    ) -> Result<(), SubmitError> {
        let hang = {
            let mut s = self.lock();
            s.submits += 1;
            s.mechanisms.push(mechanism(login));
            let (user, secret) = match login {
                SmtpLogin::Password { username, password } => (username, password),
                SmtpLogin::XOAuth2 { username, token } => (username, token),
            };
            if s.users.get(user) != Some(secret) {
                return Err(SubmitError::AuthFailed);
            }
            if let Some(err) = s.submit_failures.pop_front() {
                return Err(err);
            }
            let refused: Vec<String> = envelope
                .to
                .iter()
                .filter(|t| s.rejected.contains(&t.to_ascii_lowercase()))
                .cloned()
                .collect();
            if !refused.is_empty() {
                return Err(SubmitError::RecipientsRejected(refused));
            }
            if let Some(limit) = s.size_limit {
                if raw.len() as u64 > limit {
                    return Err(SubmitError::TooLarge);
                }
            }
            s.data_started += 1;
            std::mem::take(&mut s.hang_after_data)
        };
        if hang {
            // The answer to DATA never comes; the test kills the app meanwhile.
            std::future::pending::<()>().await;
        }
        self.lock().deliveries.push(Delivery {
            envelope: envelope.clone(),
            raw: raw.to_vec(),
        });
        Ok(())
    }

    async fn size_limit(&self, _config: &SmtpConfig) -> Result<Option<u64>, SmtpError> {
        let mut s = self.lock();
        match s.fail_next.take() {
            Some(err) => Err(err),
            None => Ok(s.size_limit),
        }
    }
}
