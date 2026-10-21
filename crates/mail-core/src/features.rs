//! Section 18 features that react to the mailbox: snooze (18.2), mute (18.5), rules and
//! Block sender (18.6), and templates (18.4). Sync tells the core about new mail and stored
//! bodies (`NewMail`, `BodyStored`); the core's own task runs rules, mute and the snooze
//! timer. Every change they make is an ordinary `ops` command, with the usual Undo.

use std::collections::{BTreeMap, HashSet};
use std::sync::Weak;
use std::time::Duration;

use mail_imap::ImapConnector;
use mail_smtp::SmtpCheck;
use mail_store::{
    AccountId, FolderId, MessageId, RuleFacts, RuleRecord, Snooze, SpecialUse, TemplateRecord,
    ThreadId,
};
use mail_sync::{ops, SyncEvent};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::core::{lock, Core, CoreEvent};
use crate::error::{AppError, ErrorCode};
use crate::views::FlagChange;

/// Setting key: the folder snoozed messages wait in, created on first use (18.2).
pub const SNOOZE_FOLDER_SETTING: &str = "snooze.folder";
const SNOOZE_FOLDER_DEFAULT: &str = "Snoozed";
/// Setting key: a reply in a snoozed thread ends the snooze early (default on, 18.2).
pub const SNOOZE_END_ON_REPLY_SETTING: &str = "snooze.end_on_reply";
/// How long a message this app moved into a folder is not new mail there.
const MOVED_IN_MEMORY: Duration = Duration::from_secs(60 * 60);

// Rules (18.6): the JSON kept in `rules.conditions_json` and `actions_json`.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    All,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    From,
    To,
    Cc,
    Subject,
    ListId,
    HasAttachment,
    Body,
}

/// One condition: the field contains the value, case-insensitively (`has_attachment`
/// ignores the value).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub field: Field,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conditions {
    #[serde(rename = "match")]
    pub match_: Match,
    pub items: Vec<Condition>,
}

/// A rule's action. Folders are named by path, so a rule for every account files into the
/// folder of that path in each one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Move { folder: String },
    Label { folder: String },
    Star,
    MarkRead,
    Mute,
    Delete,
    NeverNotify,
}

/// A rule as `rules.*` exchanges it. `id` 0 is a new rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleView {
    #[serde(default)]
    pub id: i64,
    pub account_id: Option<AccountId>,
    pub name: String,
    pub enabled: bool,
    pub conditions: Conditions,
    pub actions: Vec<Action>,
    pub stop_processing: bool,
}

impl RuleView {
    fn from_record(r: &RuleRecord) -> Option<Self> {
        Some(Self {
            id: r.id,
            account_id: r.account_id,
            name: r.name.clone(),
            enabled: r.enabled,
            conditions: serde_json::from_value(r.conditions.clone()).ok()?,
            actions: serde_json::from_value(r.actions.clone()).ok()?,
            stop_processing: r.stop_processing,
        })
    }

    fn needs_body(&self) -> bool {
        self.conditions.items.iter().any(|c| c.field == Field::Body)
    }

    /// Whether the message matches; `None` when a body condition needs a body not cached.
    fn matches(&self, f: &RuleFacts) -> Option<bool> {
        let contains = |hay: &str, needle: &str| {
            !needle.trim().is_empty() && hay.to_lowercase().contains(&needle.trim().to_lowercase())
        };
        let mut results = Vec::with_capacity(self.conditions.items.len());
        for c in &self.conditions.items {
            results.push(match c.field {
                Field::From => contains(&f.from, &c.value),
                Field::To => contains(&f.to, &c.value),
                Field::Cc => contains(&f.cc, &c.value),
                Field::Subject => contains(&f.subject, &c.value),
                Field::ListId => f.list_id.as_deref().is_some_and(|l| contains(l, &c.value)),
                Field::HasAttachment => f.has_attach,
                Field::Body => contains(f.body.as_deref()?, &c.value),
            });
        }
        if results.is_empty() {
            return Some(false);
        }
        Some(match self.conditions.match_ {
            Match::All => results.iter().all(|r| *r),
            Match::Any => results.iter().any(|r| *r),
        })
    }
}

/// A template as `templates.*` exchanges it. `id` 0 is a new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateView {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub subject: String,
    pub body_html: String,
    pub body_text: String,
}

impl From<TemplateRecord> for TemplateView {
    fn from(t: TemplateRecord) -> Self {
        Self {
            id: t.id,
            name: t.name,
            subject: t.subject,
            body_html: t.body_html,
            body_text: t.body_text,
        }
    }
}

/// Block sender: a rule that moves this address's mail to Trash and stops (18.6).
pub fn block_rule(address: &str) -> RuleView {
    RuleView {
        id: 0,
        account_id: None,
        name: format!("Block {}", address.trim()),
        enabled: true,
        conditions: Conditions {
            match_: Match::All,
            items: vec![Condition {
                field: Field::From,
                value: address.trim().to_string(),
            }],
        },
        actions: vec![Action::Delete],
        stop_processing: true,
    }
}

/// One source folder's snoozed messages: account, UIDs, Message-IDs and rows.
type SnoozeGroup = (AccountId, Vec<u32>, Vec<String>, Vec<MessageId>);

/// What one message's matching rules ask for.
#[derive(Debug, Default)]
struct Plan {
    star: bool,
    read: bool,
    mute: bool,
    labels: Vec<String>,
    move_to: Option<String>,
    delete: bool,
}

/// The core's own task: new mail and stored bodies from sync, and the snooze timer (due
/// snoozes fire within one tick of a start, section 18.2). Ends with the core.
pub(crate) async fn runner<I, S, H>(
    core: Weak<Core<I, S, H>>,
    mut rx: mpsc::UnboundedReceiver<SyncEvent>,
) where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    loop {
        let tick = match core.upgrade() {
            Some(c) => {
                c.process_snoozes().await;
                c.timing().snooze_tick
            }
            None => return,
        };
        let deadline = Instant::now() + tick;
        loop {
            let event = tokio::select! {
                e = rx.recv() => e,
                () = tokio::time::sleep_until(deadline) => break,
            };
            let Some(event) = event else { return };
            let Some(c) = core.upgrade() else { return };
            match event {
                SyncEvent::NewMail {
                    account_id,
                    folder_id,
                    message_ids,
                } => c.on_new_mail(account_id, folder_id, message_ids).await,
                SyncEvent::BodyStored { message_id, .. } => {
                    let waiting = lock(&c.rules_waiting).remove(&message_id);
                    let quiet = if waiting {
                        c.apply_rules(vec![message_id]).await
                    } else {
                        HashSet::new()
                    };
                    // Its rules done and its preview stored, it may notify (section 11).
                    let notify = if lock(&c.rules_waiting).contains(&message_id) {
                        None
                    } else {
                        lock(&c.notify_waiting).remove(&message_id)
                    };
                    if let Some(account_id) = notify {
                        c.notify_new(account_id, vec![message_id], &quiet).await;
                    }
                }
                _ => {}
            }
        }
    }
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    async fn setting_bool(&self, key: &str, default: bool) -> bool {
        match self.store.get_setting(key).await {
            Ok(Some(serde_json::Value::Bool(b))) => b,
            _ => default,
        }
    }

    /// New messages in a folder (not its first sync). In INBOX and watched folders: a reply
    /// ends a snooze early, a muted thread's message is archived, then the rules run
    /// (sections 18.2, 18.5, 18.6).
    async fn on_new_mail(&self, account_id: AccountId, folder_id: FolderId, ids: Vec<MessageId>) {
        let Ok(folder) = self.store.get_folder(folder_id).await else {
            return;
        };
        if folder.special_use != Some(SpecialUse::Inbox) && !folder.watch_idle {
            return;
        }
        let end_on_reply = self.setting_bool(SNOOZE_END_ON_REPLY_SETTING, true).await;
        let moved_in: HashSet<String> = {
            let mut map = lock(&self.moved_in);
            map.retain(|_, at| at.elapsed() < MOVED_IN_MEMORY);
            map.keys().cloned().collect()
        };
        let mut rules_for = Vec::with_capacity(ids.len());
        let mut muted = Vec::new();
        for id in ids {
            let Ok(m) = self.store.get_message(id).await else {
                continue;
            };
            // Moved here by this app (Undo, a move back): not new mail (T40).
            if moved_in.contains(&m.message_id) {
                continue;
            }
            if end_on_reply {
                if let Ok(snoozes) = self.store.thread_snoozes(m.thread_id).await {
                    for s in snoozes.into_iter().filter(|s| s.message_id != id) {
                        if let Err(e) = self.unsnooze_one(s, false).await {
                            tracing::warn!(code = ?e.code, "snooze not ended");
                        }
                    }
                }
            }
            if folder.special_use == Some(SpecialUse::Inbox)
                && self.store.thread_muted(m.thread_id).await.unwrap_or(false)
            {
                muted.push(id);
            } else {
                rules_for.push(id);
            }
        }
        if !muted.is_empty() {
            if let Err(e) = self.message_archive(&muted).await {
                tracing::warn!(code = ?e.code, account_id, "muted messages not archived");
            }
        }
        if rules_for.is_empty() {
            return;
        }
        let quiet = self.apply_rules(rules_for.clone()).await;
        self.notify_after_rules(account_id, rules_for, quiet).await;
    }

    // Snooze (18.2).

    /// `message.snooze`: moves the messages to the Snoozed folder (created on first use)
    /// and records when they come back. Returns the op for Undo.
    pub async fn message_snooze(
        &self,
        message_ids: &[MessageId],
        due_utc: i64,
    ) -> Result<i64, AppError> {
        if message_ids.is_empty() {
            return Err(AppError::invalid("no messages"));
        }
        let path = match self.store.get_setting(SNOOZE_FOLDER_SETTING).await? {
            Some(serde_json::Value::String(p)) if !p.trim().is_empty() => p,
            _ => SNOOZE_FOLDER_DEFAULT.to_string(),
        };
        let mut groups: BTreeMap<FolderId, SnoozeGroup> = BTreeMap::new();
        let mut snoozed_by_account: BTreeMap<AccountId, mail_store::Folder> = BTreeMap::new();
        for id in message_ids {
            let m = self.store.get_message(*id).await?;
            if let std::collections::btree_map::Entry::Vacant(slot) =
                snoozed_by_account.entry(m.account_id)
            {
                let folder = match self.store.find_folder(m.account_id, &path).await {
                    Ok(f) => f,
                    Err(mail_store::StoreError::NotFound) => {
                        self.folder_create(m.account_id, &path).await?;
                        self.store.find_folder(m.account_id, &path).await?
                    }
                    Err(e) => return Err(e.into()),
                };
                slot.insert(self.store.get_folder(folder).await?);
            }
            let snoozed = &snoozed_by_account[&m.account_id];
            let origin = self
                .store
                .get_snooze(m.id)
                .await?
                .map_or(m.folder_id, |s| s.origin_folder_id);
            self.store
                .add_snooze(Snooze {
                    message_id: m.id,
                    account_id: m.account_id,
                    due_utc,
                    origin_folder_id: origin,
                })
                .await?;
            if m.folder_id != snoozed.id {
                let g = groups
                    .entry(m.folder_id)
                    .or_insert_with(|| (m.account_id, Vec::new(), Vec::new(), Vec::new()));
                g.1.push(m.uid);
                g.2.push(m.message_id);
                g.3.push(m.id);
            }
        }
        let new: Vec<_> = groups
            .into_iter()
            .map(|(folder_id, (account_id, uids, mids, rows))| {
                ops::snooze_op(
                    account_id,
                    folder_id,
                    uids,
                    &snoozed_by_account[&account_id],
                    mids,
                    rows,
                )
            })
            .collect();
        if new.is_empty() {
            // Already in Snoozed: only the time changed.
            return Ok(0);
        }
        self.queue_ops(new).await
    }

    /// `message.unsnooze`: the messages go back now, without a notification.
    pub async fn message_unsnooze(&self, message_ids: &[MessageId]) -> Result<(), AppError> {
        for id in message_ids {
            if let Some(s) = self.store.get_snooze(*id).await? {
                self.unsnooze_one(s, false).await?;
            }
        }
        Ok(())
    }

    /// Fires every due snooze (section 18.2), including ones missed while the app was
    /// closed.
    pub(crate) async fn process_snoozes(&self) {
        let due = match self.store.due_snoozes(crate::accounts::now()).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "snoozes not read");
                return;
            }
        };
        for s in due {
            if let Err(e) = self.unsnooze_one(s, true).await {
                tracing::warn!(code = ?e.code, "snooze not fired");
            }
        }
    }

    /// Brings a snoozed message back to its origin folder (INBOX when that folder is gone,
    /// SPEC_GAPS G22, G77), unread. With `fire`, `snooze-due` follows once.
    async fn unsnooze_one(&self, s: Snooze, fire: bool) -> Result<(), AppError> {
        if !self.store.delete_snooze(s.message_id).await? {
            return Ok(());
        }
        let m = match self.store.get_message(s.message_id).await {
            Ok(m) => m,
            Err(mail_store::StoreError::NotFound) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let folders = self.store.list_folders(m.account_id).await?;
        let origin = folders
            .iter()
            .find(|f| f.id == s.origin_folder_id)
            .or_else(|| {
                folders
                    .iter()
                    .find(|f| f.special_use == Some(SpecialUse::Inbox))
            })
            .cloned()
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "no folder to return to"))?;
        let mut new = vec![ops::flags_op(
            m.account_id,
            m.folder_id,
            vec![m.uid],
            &[],
            &[mail_imap::Flag::Seen],
        )];
        if m.folder_id == origin.id {
            // The snooze move never ran (offline): cancel it instead.
            for op in self.store.account_ops(m.account_id).await? {
                let rows = op.args["rows"].as_array().cloned().unwrap_or_default();
                if op.args["snooze"].as_bool() == Some(true)
                    && rows.iter().any(|r| r.as_i64() == Some(m.id))
                {
                    let _ = self.store.cancel_op(op.id).await?;
                }
            }
        } else {
            new.push(ops::move_op(
                m.account_id,
                m.folder_id,
                vec![m.uid],
                &origin,
                vec![m.message_id.clone()],
            ));
        }
        self.queue_ops(new).await?;
        if fire {
            (self.sink)(CoreEvent::SnoozeDue {
                account_id: m.account_id,
                message_id: m.id,
            });
        }
        Ok(())
    }

    // Mute (18.5).

    async fn thread_account(&self, thread_id: ThreadId) -> Result<AccountId, AppError> {
        let copies = self.store.thread_copies(thread_id).await?;
        let first = copies
            .first()
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "no such thread"))?;
        Ok(self.store.get_message(first.id).await?.account_id)
    }

    /// `$Muted` on or off for a thread's messages, where the server keeps keywords.
    async fn queue_muted_keyword(&self, thread_id: ThreadId, set: bool) -> Result<(), AppError> {
        let account = self.thread_account(thread_id).await?;
        let mut by_folder: BTreeMap<FolderId, Vec<u32>> = BTreeMap::new();
        for c in self.store.thread_copies(thread_id).await? {
            by_folder.entry(c.folder_id).or_default().push(c.uid);
        }
        let new: Vec<_> = by_folder
            .into_iter()
            .map(|(folder, uids)| {
                ops::keyword_op(account, folder, uids, mail_sync::MUTED_KEYWORD, set)
            })
            .collect();
        if !new.is_empty() {
            self.queue_ops(new).await?;
        }
        Ok(())
    }

    /// `message.mute`: new messages in the thread are archived on arrival and never
    /// notify; `$Muted` is set where PERMANENTFLAGS allow (section 18.5). What is in INBOX
    /// already stays.
    pub async fn message_mute(&self, thread_id: ThreadId) -> Result<(), AppError> {
        let account = self.thread_account(thread_id).await?;
        self.store.mute_thread(account, thread_id).await?;
        self.queue_muted_keyword(thread_id, true).await
    }

    /// `message.unmute`: nothing is un-archived.
    pub async fn message_unmute(&self, thread_id: ThreadId) -> Result<(), AppError> {
        let account = self.thread_account(thread_id).await?;
        self.store.unmute_thread(account, thread_id).await?;
        self.queue_muted_keyword(thread_id, false).await
    }

    // Rules (18.6).

    /// `rules.list`.
    pub async fn rules_list(&self) -> Result<Vec<RuleView>, AppError> {
        Ok(self
            .store
            .list_rules()
            .await?
            .iter()
            .filter_map(RuleView::from_record)
            .collect())
    }

    /// `rules.save`: adds (id 0, at the end) or replaces a rule.
    pub async fn rules_save(&self, rule: RuleView) -> Result<Vec<RuleView>, AppError> {
        if rule.name.trim().is_empty() {
            return Err(AppError::invalid("a rule needs a name"));
        }
        if rule.conditions.items.is_empty() || rule.actions.is_empty() {
            return Err(AppError::invalid("a rule needs a condition and an action"));
        }
        let to_json = |v: serde_json::Result<serde_json::Value>| {
            v.map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))
        };
        self.store
            .save_rule(RuleRecord {
                id: rule.id,
                account_id: rule.account_id,
                position: 0,
                name: rule.name.trim().to_string(),
                enabled: rule.enabled,
                conditions: to_json(serde_json::to_value(&rule.conditions))?,
                actions: to_json(serde_json::to_value(&rule.actions))?,
                stop_processing: rule.stop_processing,
            })
            .await?;
        self.rules_list().await
    }

    /// `rules.delete` (also Unblock, which deletes the Block sender rule).
    pub async fn rules_delete(&self, id: i64) -> Result<Vec<RuleView>, AppError> {
        self.store.delete_rule(id).await?;
        self.rules_list().await
    }

    /// `rules.reorder`.
    pub async fn rules_reorder(&self, ids: Vec<i64>) -> Result<Vec<RuleView>, AppError> {
        self.store.reorder_rules(ids).await?;
        self.rules_list().await
    }

    /// `rules.run_now`: the rules over every cached INBOX message now. A rule never runs
    /// twice on one message.
    pub async fn rules_run_now(&self) -> Result<Vec<RuleView>, AppError> {
        let mut ids = Vec::new();
        for a in self.store.list_accounts().await? {
            for f in self.store.list_folders(a.id).await? {
                if f.special_use == Some(SpecialUse::Inbox) {
                    let mut cursor = None;
                    loop {
                        let page = self.store.folder_page(f.id, cursor, 500).await?;
                        ids.extend(page.rows.iter().map(|r| r.id));
                        match page.next {
                            Some(c) => cursor = Some(c),
                            None => break,
                        }
                    }
                }
            }
        }
        self.apply_rules(ids).await;
        self.rules_list().await
    }

    /// Runs the rules once per message, in order (section 18.6). Messages whose rules need
    /// a body wait until it is stored. Actions are batched into ops per folder. Returns the
    /// messages a Never notify rule matched (section 11).
    pub(crate) async fn apply_rules(&self, ids: Vec<MessageId>) -> HashSet<MessageId> {
        let mut quiet = HashSet::new();
        let rules: Vec<RuleView> = match self.rules_list().await {
            Ok(r) => r.into_iter().filter(|r| r.enabled).collect(),
            Err(_) => return quiet,
        };
        if rules.is_empty() || ids.is_empty() {
            return quiet;
        }
        let Ok(facts) = self.store.rule_facts(ids).await else {
            return quiet;
        };
        let mut marks = Vec::new();
        let mut plans: Vec<(RuleFacts, Plan)> = Vec::new();
        for f in facts {
            if f.hidden {
                continue;
            }
            let applied = self.store.rules_applied(f.id).await.unwrap_or_default();
            let mut plan = Plan::default();
            let mut any = false;
            for rule in &rules {
                if rule.account_id.is_some_and(|a| a != f.account_id) {
                    continue;
                }
                if applied.contains(&rule.id) {
                    if rule.actions.contains(&Action::NeverNotify) {
                        quiet.insert(f.id);
                    }
                    if rule.stop_processing {
                        break;
                    }
                    continue;
                }
                if rule.needs_body() && f.body.is_none() {
                    lock(&self.rules_waiting).insert(f.id);
                    break;
                }
                if rule.matches(&f) != Some(true) {
                    continue;
                }
                any = true;
                marks.push((f.id, rule.id));
                for a in &rule.actions {
                    match a {
                        Action::Star => plan.star = true,
                        Action::MarkRead => plan.read = true,
                        Action::Mute => plan.mute = true,
                        Action::Label { folder } => plan.labels.push(folder.clone()),
                        Action::Move { folder } => plan.move_to = Some(folder.clone()),
                        Action::Delete => plan.delete = true,
                        Action::NeverNotify => {
                            quiet.insert(f.id);
                        }
                    }
                }
                if rule.stop_processing {
                    break;
                }
            }
            if any {
                plans.push((f, plan));
            }
        }
        if !marks.is_empty() {
            if let Err(e) = self.store.mark_rules_applied(marks).await {
                tracing::warn!(error = %e, "rules not recorded");
            }
        }
        if let Err(e) = self.carry_out(plans).await {
            tracing::warn!(code = ?e.code, "rule actions not queued");
        }
        quiet
    }

    /// Queues the rules' actions, batched: flags, labels and mute first, then the move or
    /// delete.
    async fn carry_out(&self, plans: Vec<(RuleFacts, Plan)>) -> Result<(), AppError> {
        let ids_where = |pred: &dyn Fn(&Plan) -> bool| -> Vec<MessageId> {
            plans
                .iter()
                .filter(|(_, p)| pred(p))
                .map(|(f, _)| f.id)
                .collect()
        };
        for (change, ids) in [
            (
                FlagChange {
                    seen: Some(true),
                    flagged: Some(true),
                },
                ids_where(&|p| p.star && p.read),
            ),
            (
                FlagChange {
                    seen: None,
                    flagged: Some(true),
                },
                ids_where(&|p| p.star && !p.read),
            ),
            (
                FlagChange {
                    seen: Some(true),
                    flagged: None,
                },
                ids_where(&|p| p.read && !p.star),
            ),
        ] {
            if !ids.is_empty() {
                self.message_set_flags(&ids, change).await?;
            }
        }
        let mut labels: BTreeMap<(AccountId, String), Vec<MessageId>> = BTreeMap::new();
        let mut moves: BTreeMap<(AccountId, String), Vec<MessageId>> = BTreeMap::new();
        let mut deletes = Vec::new();
        for (f, p) in &plans {
            for l in &p.labels {
                labels
                    .entry((f.account_id, l.clone()))
                    .or_default()
                    .push(f.id);
            }
            if p.mute {
                let _ = self.store.mute_thread(f.account_id, f.thread_id).await;
            }
            if p.delete {
                deletes.push(f.id);
            } else if let Some(m) = &p.move_to {
                moves
                    .entry((f.account_id, m.clone()))
                    .or_default()
                    .push(f.id);
            }
        }
        for ((account, path), ids) in labels {
            if let Ok(folder) = self.store.find_folder(account, &path).await {
                if let Err(e) = self.message_add_label(&ids, folder).await {
                    tracing::warn!(code = ?e.code, "rule label not added");
                }
            }
        }
        for ((account, path), ids) in moves {
            if let Ok(folder) = self.store.find_folder(account, &path).await {
                self.message_move(&ids, folder).await?;
            }
        }
        if !deletes.is_empty() {
            self.message_delete(&deletes, None).await?;
        }
        Ok(())
    }

    // Templates (18.4).

    /// `templates.list`.
    pub async fn templates_list(&self) -> Result<Vec<TemplateView>, AppError> {
        Ok(self
            .store
            .list_templates()
            .await?
            .into_iter()
            .map(TemplateView::from)
            .collect())
    }

    /// `templates.save`: adds (id 0) or replaces a template. The HTML goes through the
    /// outgoing allowlist (SPEC_GAPS G67).
    pub async fn templates_save(&self, t: TemplateView) -> Result<Vec<TemplateView>, AppError> {
        if t.name.trim().is_empty() {
            return Err(AppError::invalid("a template needs a name"));
        }
        self.store
            .save_template(TemplateRecord {
                id: t.id,
                name: t.name.trim().to_string(),
                subject: t.subject,
                body_html: mail_render::sanitize_outgoing(&t.body_html),
                body_text: t.body_text,
                updated_at: crate::accounts::now(),
            })
            .await?;
        self.templates_list().await
    }

    /// `templates.delete`.
    pub async fn templates_delete(&self, id: i64) -> Result<Vec<TemplateView>, AppError> {
        self.store.delete_template(id).await?;
        self.templates_list().await
    }

    /// `draft.apply_template` (18.4): fills the draft's subject when it is empty and
    /// returns the template, sanitized; the compose window inserts the body at the cursor
    /// (SPEC_GAPS G77).
    pub async fn draft_apply_template(
        &self,
        draft_id: mail_store::DraftId,
        template_id: i64,
    ) -> Result<TemplateView, AppError> {
        let t = self.store.get_template(template_id).await?;
        let draft = self.store.get_draft(draft_id).await?;
        let mut content: crate::compose::DraftContent =
            serde_json::from_value(draft.content.clone())
                .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?;
        if content.subject.trim().is_empty() && !t.subject.trim().is_empty() {
            content.subject = t.subject.clone();
            self.draft_save(draft_id, content).await?;
        }
        Ok(TemplateView {
            body_html: mail_render::sanitize_outgoing(&t.body_html),
            ..TemplateView::from(t)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> RuleFacts {
        RuleFacts {
            id: 1,
            account_id: 1,
            folder_id: 1,
            uid: 1,
            thread_id: 1,
            message_id: "x".into(),
            from: "News Desk news@list.test".into(),
            to: "[[\"Me\",\"me@x.test\"]]".into(),
            cc: "[]".into(),
            subject: "Weekly digest".into(),
            list_id: Some("weekly.list.test".into()),
            has_attach: false,
            body: None,
            hidden: false,
        }
    }

    #[test]
    fn conditions_match_case_insensitively_and_bodies_wait() {
        let mut rule = block_rule("NEWS@list.test");
        assert_eq!(rule.matches(&facts()), Some(true));
        rule.conditions.items.push(Condition {
            field: Field::Body,
            value: "unsubscribe".into(),
        });
        assert_eq!(rule.matches(&facts()), None, "waits for the body");
        rule.conditions.match_ = Match::Any;
        assert_eq!(rule.matches(&facts()), None);
        let mut f = facts();
        f.body = Some("To unsubscribe, click".into());
        rule.conditions.match_ = Match::All;
        assert_eq!(rule.matches(&f), Some(true));
        let list = RuleView {
            conditions: Conditions {
                match_: Match::All,
                items: vec![Condition {
                    field: Field::ListId,
                    value: "weekly.list".into(),
                }],
            },
            ..rule.clone()
        };
        assert_eq!(list.matches(&facts()), Some(true));
        let empty = RuleView {
            conditions: Conditions {
                match_: Match::Any,
                items: vec![Condition {
                    field: Field::Subject,
                    value: "  ".into(),
                }],
            },
            ..rule
        };
        assert_eq!(
            empty.matches(&facts()),
            Some(false),
            "an empty value matches nothing"
        );
    }
}
