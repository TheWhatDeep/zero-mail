//! Notifications (section 11): which new messages notify, and the coalescing of bursts.
//! Core decides and emits `notify`; the app shows the OS notification only while its window
//! is unfocused or hidden.
//!
//! A message notifies when it is new mail in INBOX or a watched folder (never a first sync,
//! never one this app moved there), still unread and visible after mute and the rules, no
//! rule with Never notify matched it, its INTERNALDATE is within the last 30 minutes, and
//! notifications are on for its account. More than 3 within 10 s in one account coalesce
//! into one "N new messages" notification.

use std::collections::HashMap;
use std::time::Duration;

use mail_imap::ImapConnector;
use mail_smtp::SmtpCheck;
use mail_store::{AccountId, Flags, MessageId};
use tokio::time::Instant;

use crate::core::{lock, Core, CoreEvent};
use crate::settings::{NOTIFY_ACCOUNT_PREFIX, NOTIFY_PREVIEW};

/// Only messages received this recently notify (section 11).
pub const RECENT: Duration = Duration::from_secs(30 * 60);
/// The coalescing window (section 11).
pub const WINDOW: Duration = Duration::from_secs(10);
/// At most this many single notifications per account per window.
pub const MAX_SINGLE: usize = 3;
/// With the preview on, how long a notification waits for the body before it shows
/// without one.
pub const PREVIEW_WAIT: Duration = Duration::from_secs(5);

/// One message's notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub message_id: MessageId,
    pub sender: String,
    pub subject: String,
    /// The list preview, only when the preview setting is on.
    pub preview: Option<String>,
}

/// What to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    One(AccountId, Notice),
    /// "N new messages in <account>"; the message is the newest of them.
    Many {
        account_id: AccountId,
        message_id: MessageId,
        count: u32,
    },
}

#[derive(Debug)]
struct Window {
    start: Instant,
    shown: usize,
    /// A burst was coalesced in this window: what arrives until it ends waits for one
    /// notification at its end.
    coalesced: bool,
    held: Vec<Notice>,
}

/// Per-account coalescing (section 11). Pure: the caller passes the time.
#[derive(Debug, Default)]
pub struct Coalescer {
    windows: HashMap<AccountId, Window>,
}

fn many(account_id: AccountId, notices: &[Notice]) -> Option<Shown> {
    let newest = notices.iter().map(|n| n.message_id).max()?;
    Some(Shown::Many {
        account_id,
        message_id: newest,
        count: u32::try_from(notices.len()).unwrap_or(u32::MAX),
    })
}

impl Coalescer {
    /// New notices for an account. Returns what to show now, and when to call
    /// [`Coalescer::flush`] if some are held.
    pub fn push(
        &mut self,
        now: Instant,
        account_id: AccountId,
        notices: Vec<Notice>,
    ) -> (Vec<Shown>, Option<Instant>) {
        let mut out = self.flush_account(now, account_id);
        if notices.is_empty() {
            return (out, None);
        }
        let w = self.windows.entry(account_id).or_insert(Window {
            start: now,
            shown: 0,
            coalesced: false,
            held: Vec::new(),
        });
        if now.duration_since(w.start) >= WINDOW {
            *w = Window {
                start: now,
                shown: 0,
                coalesced: false,
                held: Vec::new(),
            };
        }
        if w.coalesced {
            w.held.extend(notices);
            return (out, Some(w.start + WINDOW));
        }
        if w.shown + notices.len() <= MAX_SINGLE {
            w.shown += notices.len();
            out.extend(notices.into_iter().map(|n| Shown::One(account_id, n)));
        } else {
            w.coalesced = true;
            out.extend(many(account_id, &notices));
        }
        (out, None)
    }

    /// Shows what waited for the end of its window.
    pub fn flush(&mut self, now: Instant) -> Vec<Shown> {
        let accounts: Vec<AccountId> = self.windows.keys().copied().collect();
        accounts
            .into_iter()
            .flat_map(|a| self.flush_account(now, a))
            .collect()
    }

    /// When the earliest held notices are due.
    pub fn next_due(&self) -> Option<Instant> {
        self.windows
            .values()
            .filter(|w| !w.held.is_empty())
            .map(|w| w.start + WINDOW)
            .min()
    }

    fn flush_account(&mut self, now: Instant, account_id: AccountId) -> Vec<Shown> {
        let Some(w) = self.windows.get_mut(&account_id) else {
            return Vec::new();
        };
        if w.held.is_empty() || now.duration_since(w.start) < WINDOW {
            return Vec::new();
        }
        let held = std::mem::take(&mut w.held);
        // A new window starts with this notification, still coalescing: a steady stream
        // shows one notification per window.
        w.start = now;
        w.shown = 0;
        w.coalesced = true;
        many(account_id, &held).into_iter().collect()
    }
}

/// The name a notification shows for the sender.
fn sender_of(name: &str, addr: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        addr.to_string()
    } else {
        name.to_string()
    }
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    /// After the rules ran on new mail: a message whose rules wait for its body notifies
    /// when they have run, and with the preview on, one whose body is not stored yet waits
    /// for it (at most [`PREVIEW_WAIT`]). The rest notify now.
    pub(crate) async fn notify_after_rules(
        &self,
        account_id: AccountId,
        ids: Vec<MessageId>,
        quiet: std::collections::HashSet<MessageId>,
    ) {
        let preview = self.setting_on(NOTIFY_PREVIEW).await;
        let (mut now, mut later, mut for_body) = (Vec::new(), Vec::new(), Vec::new());
        for id in ids.into_iter().filter(|id| !quiet.contains(id)) {
            if lock(&self.rules_waiting).contains(&id) {
                later.push(id);
            } else if preview
                && self
                    .store
                    .get_message(id)
                    .await
                    .is_ok_and(|m| m.body_state == mail_store::BodyState::None)
            {
                for_body.push(id);
            } else {
                now.push(id);
            }
        }
        lock(&self.notify_waiting).extend(
            later
                .into_iter()
                .chain(for_body.iter().copied())
                .map(|id| (id, account_id)),
        );
        if !for_body.is_empty() {
            if let Some(me) = self.me.get().cloned() {
                tokio::spawn(async move {
                    tokio::time::sleep(PREVIEW_WAIT).await;
                    let Some(core) = me.upgrade() else { return };
                    let due: Vec<MessageId> = {
                        let mut waiting = lock(&core.notify_waiting);
                        for_body
                            .into_iter()
                            .filter(|id| waiting.remove(id).is_some())
                            .collect()
                    };
                    core.notify_new(account_id, due, &std::collections::HashSet::new())
                        .await;
                });
            }
        }
        self.notify_new(account_id, now, &quiet).await;
    }

    /// Notifies for new mail that passed mute and the rules (`quiet`: matched Never notify).
    pub(crate) async fn notify_new(
        &self,
        account_id: AccountId,
        ids: Vec<MessageId>,
        quiet: &std::collections::HashSet<MessageId>,
    ) {
        if ids.is_empty() {
            return;
        }
        if !self
            .setting_on(&format!("{NOTIFY_ACCOUNT_PREFIX}{account_id}"))
            .await
        {
            return;
        }
        match self.store.get_account(account_id).await {
            Ok(a) if a.config.enabled => {}
            _ => return,
        }
        let preview = self.setting_on(NOTIFY_PREVIEW).await;
        let recent_since = crate::accounts::now() - i64::try_from(RECENT.as_secs()).unwrap_or(0);
        let mut notices = Vec::new();
        for id in ids {
            if quiet.contains(&id) {
                continue;
            }
            // Rules and mute have run: their flags and moves are already local.
            let Ok(m) = self.store.get_message(id).await else {
                continue;
            };
            if m.flags.contains(Flags::SEEN)
                || m.flags.contains(Flags::DELETED)
                || m.internal_utc < recent_since
            {
                continue;
            }
            if self.store.thread_muted(m.thread_id).await.unwrap_or(false) {
                continue;
            }
            notices.push(Notice {
                message_id: id,
                sender: sender_of(&m.from.name, &m.from.addr),
                subject: m.subject,
                preview: preview.then_some(m.preview).filter(|p| !p.is_empty()),
            });
        }
        let (shown, flush_at) = lock(&self.notices).push(Instant::now(), account_id, notices);
        self.show(shown).await;
        if flush_at.is_some() {
            self.schedule_flush();
        }
    }

    /// One timer, for the earliest held notices; it runs until nothing is held.
    fn schedule_flush(&self) {
        let Some(me) = self.me.get().cloned() else {
            return;
        };
        let Some(mut at) = lock(&self.notices).next_due() else {
            return;
        };
        let mut timer = lock(&self.notice_timer);
        if let Some(old) = timer.take() {
            old.abort();
        }
        *timer = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep_until(at).await;
                let Some(core) = me.upgrade() else { return };
                let (shown, next) = {
                    let mut c = lock(&core.notices);
                    (c.flush(Instant::now()), c.next_due())
                };
                core.show(shown).await;
                match next {
                    Some(n) => at = n,
                    None => return,
                }
            }
        }));
    }

    async fn show(&self, shown: Vec<Shown>) {
        for s in shown {
            let event = match s {
                Shown::One(account_id, n) => CoreEvent::Notify {
                    account_id,
                    message_id: n.message_id,
                    sender: n.sender,
                    subject: n.subject,
                    preview: n.preview,
                    count: 1,
                    account_name: self.account_name(account_id).await,
                },
                Shown::Many {
                    account_id,
                    message_id,
                    count,
                } => CoreEvent::Notify {
                    account_id,
                    message_id,
                    sender: String::new(),
                    subject: String::new(),
                    preview: None,
                    count,
                    account_name: self.account_name(account_id).await,
                },
            };
            (self.sink)(event);
        }
    }

    async fn account_name(&self, account_id: AccountId) -> String {
        match self.store.get_account(account_id).await {
            Ok(a) if !a.config.name.trim().is_empty() => a.config.name,
            Ok(a) => a.config.email,
            Err(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: MessageId) -> Notice {
        Notice {
            message_id: id,
            sender: "Ann".into(),
            subject: "Hi".into(),
            preview: None,
        }
    }

    fn singles(shown: &[Shown]) -> usize {
        shown.iter().filter(|s| matches!(s, Shown::One(..))).count()
    }

    #[test]
    fn three_singles_then_one_burst_notification() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        let (s, at) = c.push(t0, 1, vec![n(1)]);
        assert_eq!((singles(&s), at), (1, None));
        let (s, _) = c.push(t0 + Duration::from_secs(1), 1, vec![n(2), n(3)]);
        assert_eq!(singles(&s), 2);
        // The fourth within 10 s: one "N new messages".
        let (s, at) = c.push(t0 + Duration::from_secs(2), 1, vec![n(4)]);
        assert_eq!(
            s,
            vec![Shown::Many {
                account_id: 1,
                message_id: 4,
                count: 1
            }]
        );
        assert_eq!(at, None);
        // More in the same window wait for its end, then show as one.
        let (s, at) = c.push(t0 + Duration::from_secs(3), 1, vec![n(5), n(6)]);
        assert!(s.is_empty());
        assert_eq!(at, Some(t0 + WINDOW));
        assert!(c.flush(t0 + Duration::from_secs(9)).is_empty());
        assert_eq!(
            c.flush(t0 + WINDOW),
            vec![Shown::Many {
                account_id: 1,
                message_id: 6,
                count: 2
            }]
        );
        // A quiet window later, singles again.
        let (s, _) = c.push(t0 + Duration::from_secs(25), 1, vec![n(7)]);
        assert_eq!(singles(&s), 1);
    }

    #[test]
    fn a_burst_in_one_batch_is_one_notification() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        let (s, _) = c.push(t0, 1, (1..=5).map(n).collect());
        assert_eq!(
            s,
            vec![Shown::Many {
                account_id: 1,
                message_id: 5,
                count: 5
            }]
        );
        // Accounts coalesce separately.
        let (s, _) = c.push(t0, 2, vec![n(9)]);
        assert_eq!(singles(&s), 1);
    }

    #[test]
    fn sender_falls_back_to_the_address() {
        assert_eq!(sender_of("  ", "a@x.org"), "a@x.org");
        assert_eq!(sender_of("Ann", "a@x.org"), "Ann");
    }
}
