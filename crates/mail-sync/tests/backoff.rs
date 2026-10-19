//! T08 and the section 6 retry rules, on paused tokio time so minutes pass instantly and
//! intervals are exact. Waits here never use a timer: with the clock paused, a timer would
//! be advanced to while the store works on its own thread.
#![cfg(test)]

mod common;

use std::time::Duration;

use common::*;
use mail_imap::fake::{FakeServer, Op};
use mail_imap::ImapError;
use mail_sync::{AccountState, SyncError, SyncEvent};

const USER: &str = "me@test.local";
const PASS: &str = "pw";

struct Setup {
    _dir: TempDir,
    store: mail_store::Store,
    account: i64,
    server: FakeServer,
    rec: Recording,
    watch: Watch,
    handle: mail_sync::AccountHandle,
}

async fn setup(blocked: usize, error: ImapError) -> Setup {
    let dir = TempDir::new("backoff");
    let store = dir.store();
    let account = add_account(&store, USER).await;
    let server = FakeServer::new(&["IMAP4rev1", "IDLE"]);
    server.add_user(USER, PASS);
    for _ in 0..blocked {
        server.fail_next(Op::Connect, error.clone());
    }
    let logins = Logins::default();
    logins.set(USER, PASS);
    let rec = Recording::new(server.clone());
    let (events, watch) = Watch::new();
    let handle = spawn(&store, rec.clone(), &logins, &events, account);
    Setup {
        _dir: dir,
        store,
        account,
        server,
        rec,
        watch,
        handle,
    }
}

impl Setup {
    async fn next_backoff(&mut self) -> (Duration, Option<SyncError>) {
        let account = self.account;
        let e = self
            .watch
            .until(None, |e| {
                matches!(e, SyncEvent::AccountState { account_id, state: AccountState::Backoff, .. } if *account_id == account)
            })
            .await;
        match e {
            SyncEvent::AccountState {
                retry_in: Some(d),
                last_error,
                ..
            } => (d, last_error),
            other => unreachable!("unexpected event {other:?}"),
        }
    }

    async fn state(&mut self, state: AccountState) -> SyncEvent {
        self.watch.state_within(self.account, state, None).await
    }
}

fn within(d: Duration, base_secs: u64) -> bool {
    let base = Duration::from_secs(base_secs);
    d >= base.mul_f64(0.8) && d <= base.mul_f64(1.2)
}

#[tokio::test(start_paused = true)]
async fn t08_blocked_port_backs_off_from_2s_doubling_to_5min_and_recovers() {
    let refused = ImapError::Network("connection refused".into());
    let mut s = setup(10, refused.clone()).await;

    let mut delays = Vec::new();
    for _ in 0..10 {
        let (delay, error) = s.next_backoff().await;
        assert_eq!(
            error,
            Some(SyncError::Imap(refused.clone())),
            "the UI sees why"
        );
        delays.push(delay);
    }
    let bases = [2, 4, 8, 16, 32, 64, 128, 256, 300, 300];
    for (delay, base) in delays.iter().zip(bases) {
        assert!(within(*delay, base), "{delay:?} is not {base} s ± 20%");
    }

    // Unblocked: the 11th attempt connects and the account recovers.
    s.state(AccountState::Idle).await;
    let attempts = s.rec.attempts();
    assert!(attempts.len() >= 11);
    for i in 0..10 {
        let gap = attempts[i + 1] - attempts[i];
        let diff = gap.abs_diff(delays[i]);
        assert!(
            diff < Duration::from_millis(5),
            "attempt {i}: waited {gap:?}, announced {:?}",
            delays[i]
        );
    }
    let states: Vec<AccountState> = s
        .watch
        .seen
        .iter()
        .filter_map(|e| match e {
            SyncEvent::AccountState { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(
        &states[..3],
        &[
            AccountState::Connecting,
            AccountState::Backoff,
            AccountState::Connecting
        ]
    );

    // Healthy for over 60 s, then the connection drops: the backoff starts again at 2 s.
    tokio::time::sleep(Duration::from_secs(61)).await;
    let inbox = folder_id(&s.store, s.account, "INBOX").await;
    s.server.fail_next(Op::Select, refused.clone());
    s.handle.refresh(inbox);
    let (delay, _) = s.next_backoff().await;
    assert!(within(delay, 2), "reset after a healthy minute: {delay:?}");
    s.state(AccountState::Idle).await;
    s.handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn throttling_pauses_ten_minutes() {
    let mut s = setup(1, ImapError::Throttled).await;
    let (delay, error) = s.next_backoff().await;
    assert_eq!(delay, Duration::from_secs(600));
    assert_eq!(error, Some(SyncError::Imap(ImapError::Throttled)));
    s.state(AccountState::Idle).await;
    let a = s.rec.attempts();
    assert_eq!(a[1] - a[0], Duration::from_secs(600));
    s.handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn bye_reconnects_once_at_once_then_backs_off() {
    let mut s = setup(0, ImapError::Bye).await;
    s.state(AccountState::Idle).await;
    let inbox = folder_id(&s.store, s.account, "INBOX").await;
    let before = s.rec.attempts().len();

    // The server says BYE mid-command; the immediate reconnect is refused.
    s.server.fail_next(Op::Select, ImapError::Bye);
    s.server
        .fail_next(Op::Connect, ImapError::Network("refused".into()));
    s.handle.refresh(inbox);
    let (delay, error) = s.next_backoff().await;
    assert_eq!(
        error,
        Some(SyncError::Imap(ImapError::Network("refused".into())))
    );
    let attempts = s.rec.attempts();
    assert_eq!(attempts.len(), before + 1, "one immediate reconnect");
    s.state(AccountState::Idle).await;
    let attempts = s.rec.attempts();
    // The worker after the delay, then the new session's IDLE connection.
    assert_eq!(attempts.len(), before + 3);
    let gap = attempts[before + 1] - attempts[before];
    assert!(gap.abs_diff(delay) < Duration::from_millis(5));
    s.handle.shutdown().await;
}
