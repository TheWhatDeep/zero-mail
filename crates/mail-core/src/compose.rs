//! Sending (section 7): compose, drafts, the outbox, identities and contacts.
//!
//! - **Compose** prefills a draft: Reply goes to Reply-To if present, else From; Reply all
//!   adds To and Cc minus every identity of the account, de-duplicated case-insensitively;
//!   `Re:` and `Fwd:` never stack; the quote is the original's plain text (SPEC_GAPS G67).
//! - **Drafts** are saved locally by the composer and uploaded 30 s after the last change and
//!   when the window closes, through `ops` (G66). Attached files are copied into the app
//!   data directory at attach time.
//! - **Send** builds the message, checks the size where the servers can be asked (G65),
//!   and queues it with the undo window. The outbox runner sends due items, then files the
//!   Sent copy, marks the original Answered or Forwarded, removes the draft and counts the
//!   recipients as contacts.

use std::path::PathBuf;
use std::sync::Weak;
use std::time::Duration;

use mail_imap::{Flag, ImapConnector};
use mail_outbox::Transition;
use mail_smtp::{
    MailboxSpec, OutgoingAttachment, OutgoingMessage, SmtpCheck, SmtpConfig, SmtpLogin, SmtpSubmit,
    SubmitError,
};
use mail_store::{
    Account, AccountId, DraftId, Folder, IdentityId, MessageId, NewIdentity, NewOutbox, OutboxId,
    OutboxItem, OutboxState, SpecialUse, StoreError,
};
use mail_sync::ops;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::{Core, CoreEvent};
use crate::error::{AppError, ErrorCode};
use crate::views::AddressView;

/// The undo window and the draft upload delay (section 7). Tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub undo_window: Duration,
    pub draft_upload: Duration,
    /// How often the outbox runner looks for due items.
    pub outbox_tick: Duration,
    /// A scheduled send this late is marked Sent late (section 18.3; SPEC_GAPS G77).
    pub late_after: Duration,
    /// How often due snoozes are looked for (section 18.2: within 10 s of a start).
    pub snooze_tick: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            undo_window: Duration::from_secs(10),
            draft_upload: Duration::from_secs(30),
            outbox_tick: Duration::from_secs(1),
            late_after: Duration::from_secs(60),
            snooze_tick: Duration::from_secs(5),
        }
    }
}

/// A file attached to a draft, copied into the app data directory at attach time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DraftAttachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

/// What the composer quotes; the page formats the date in local time (section 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Quote {
    pub date_utc: i64,
    pub from: AddressView,
    pub to: Vec<AddressView>,
    pub subject: String,
    pub text: String,
    /// An inline forward: the forwarded-message header block, not "wrote:".
    pub forwarded: bool,
}

/// A draft's content (`drafts.content_json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DraftContent {
    pub identity_id: Option<IdentityId>,
    pub to: Vec<AddressView>,
    pub cc: Vec<AddressView>,
    pub bcc: Vec<AddressView>,
    pub subject: String,
    /// The composer's HTML.
    pub html: String,
    /// The plain text (the composer's text, or the plain version of the HTML).
    pub text: String,
    /// Send plain text only (section 7).
    pub plain_only: bool,
    pub attachments: Vec<DraftAttachment>,
    /// Forward as attachment: the original message (section 7).
    pub forward_attachment: Option<MessageId>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// The message answered or forwarded, for `\Answered` or `$Forwarded`.
    pub original: Option<MessageId>,
    /// `new`, `reply`, `reply_all` or `forward`.
    pub mode: String,
    /// Set by compose; the composer turns it into its first HTML.
    pub quote: Option<Quote>,
    /// Kept across uploads, so the server copy can be found again.
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityView {
    pub id: Option<IdentityId>,
    pub account_id: AccountId,
    pub name: String,
    pub address: String,
    pub reply_to: Option<String>,
    pub is_default: bool,
}

/// What a compose window opens with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftView {
    pub draft_id: DraftId,
    pub account_id: AccountId,
    pub content: DraftContent,
    pub identities: Vec<IdentityView>,
    pub signature_html: String,
    pub signature_text: String,
}

impl DraftView {
    /// The view as JSON, for the compose window's start-up script (SPEC_GAPS G63).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "null".into())
    }
}

/// One `outbox.list` row (SPEC_GAPS G69).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutboxView {
    pub id: OutboxId,
    pub account_id: AccountId,
    pub subject: String,
    pub to: Vec<String>,
    pub state: &'static str,
    pub error: Option<Value>,
    pub send_after_utc: i64,
    pub created_at: i64,
    pub draft_id: Option<DraftId>,
    /// Sent late: missed while the app was closed (section 18.3).
    pub late: bool,
}

/// Prefixes that never stack (section 7).
fn prefixed(prefix: &str, subject: &str) -> String {
    let s = subject.trim();
    let lower = s.to_ascii_lowercase();
    if lower.starts_with(&format!("{}:", prefix.to_ascii_lowercase())) {
        s.to_string()
    } else {
        format!("{prefix}: {s}")
    }
}

/// Addresses minus `own`, de-duplicated case-insensitively (section 7, T17).
fn without(list: Vec<AddressView>, own: &[String], seen: &mut Vec<String>) -> Vec<AddressView> {
    list.into_iter()
        .filter(|a| {
            let key = a.addr.trim().to_ascii_lowercase();
            if key.is_empty() || own.contains(&key) || seen.contains(&key) {
                return false;
            }
            seen.push(key);
            true
        })
        .collect()
}

/// A content type from a file name; anything unknown is `application/octet-stream`.
pub fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "json" => "application/json",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ics" => "text/calendar",
        "eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
}

/// Standard base64 with padding, for a draft's raw message inside its op (G66).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(char::from(T[((n >> (18 - 6 * k)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn spec(a: &AddressView) -> MailboxSpec {
    MailboxSpec {
        name: a.name.clone(),
        addr: a.addr.clone(),
    }
}

fn identity_view(i: &mail_store::Identity) -> IdentityView {
    IdentityView {
        id: Some(i.id),
        account_id: i.account_id,
        name: i.identity.name.clone(),
        address: i.identity.address.clone(),
        reply_to: i.identity.reply_to.clone(),
        is_default: i.identity.is_default,
    }
}

pub(crate) fn smtp_config(account: &Account) -> SmtpConfig {
    let a = &account.config;
    SmtpConfig {
        host: a.smtp_host.clone(),
        port: a.smtp_port,
        security: match a.smtp_security {
            mail_store::Security::Tls => mail_smtp::Security::Tls,
            mail_store::Security::StartTls => mail_smtp::Security::StartTls,
        },
        pinned_cert_sha256: a
            .pinned_cert_sha256
            .as_deref()
            .and_then(mail_imap::parse_fingerprint),
        connect_timeout: mail_imap::CONNECT_TIMEOUT,
    }
}

fn state_name(s: OutboxState) -> &'static str {
    s.as_sql()
}

fn bad(message: &str) -> AppError {
    AppError::invalid(message)
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    fn drafts_dir(&self, draft_id: DraftId) -> PathBuf {
        self.data_dir.join("drafts").join(draft_id.to_string())
    }

    async fn own_addresses(&self, account_id: AccountId) -> Result<Vec<String>, AppError> {
        let account = self.store.get_account(account_id).await?;
        let mut own: Vec<String> = self
            .store
            .list_identities(account_id)
            .await?
            .iter()
            .map(|i| i.identity.address.trim().to_ascii_lowercase())
            .collect();
        own.push(account.config.email.trim().to_ascii_lowercase());
        Ok(own)
    }

    async fn new_draft(
        &self,
        account_id: AccountId,
        mut content: DraftContent,
    ) -> Result<DraftView, AppError> {
        let account = self.store.get_account(account_id).await?;
        let identities = self.store.list_identities(account_id).await?;
        if content.identity_id.is_none() {
            content.identity_id = identities.first().map(|i| i.id);
        }
        let domain = identities
            .first()
            .map(|i| i.identity.address.clone())
            .unwrap_or(account.config.email.clone());
        let domain = domain
            .rsplit_once('@')
            .map_or("localhost", |(_, d)| d)
            .to_string();
        content.message_id = format!("{}@{domain}", uuid_like());
        let value = serde_json::to_value(&content)
            .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?;
        let draft_id = self
            .store
            .insert_draft(account_id, value, crate::accounts::now())
            .await?;
        self.draft_view(draft_id).await
    }

    /// The compose window's starting point for a draft.
    pub async fn draft_view(&self, draft_id: DraftId) -> Result<DraftView, AppError> {
        let draft = self.store.get_draft(draft_id).await?;
        let account = self.store.get_account(draft.account_id).await?;
        let content: DraftContent = serde_json::from_value(draft.content).unwrap_or_default();
        Ok(DraftView {
            draft_id,
            account_id: draft.account_id,
            content,
            identities: self
                .store
                .list_identities(draft.account_id)
                .await?
                .iter()
                .map(identity_view)
                .collect(),
            signature_html: account.config.signature_html,
            signature_text: account.config.signature_text,
        })
    }

    /// `compose.new`: an empty draft on `account_id`, optionally to one address (mailto).
    pub async fn compose_new(
        &self,
        account_id: AccountId,
        to: Option<String>,
    ) -> Result<DraftView, AppError> {
        let to = to
            .filter(|t| !t.trim().is_empty())
            .map(|t| {
                vec![AddressView {
                    name: String::new(),
                    addr: t.trim().to_string(),
                }]
            })
            .unwrap_or_default();
        self.new_draft(
            account_id,
            DraftContent {
                to,
                mode: "new".into(),
                ..DraftContent::default()
            },
        )
        .await
    }

    /// `compose.reply` and `compose.reply_all` (section 7, T17).
    pub async fn compose_reply(
        &self,
        message_id: MessageId,
        all: bool,
    ) -> Result<DraftView, AppError> {
        let m = self.store.get_message(message_id).await?;
        let own = self.own_addresses(m.account_id).await?;
        let body = self.store.get_body(message_id).await?;
        let first: Vec<AddressView> = if m.reply_to.is_empty() {
            vec![AddressView::from(&m.from)]
        } else {
            m.reply_to.iter().map(AddressView::from).collect()
        };
        let mut seen = Vec::new();
        let (to, cc) = if all {
            let mut to_list = first;
            to_list.extend(m.to.iter().map(AddressView::from));
            let to = without(to_list, &own, &mut seen);
            let cc = without(
                m.cc.iter().map(AddressView::from).collect(),
                &own,
                &mut seen,
            );
            (to, cc)
        } else {
            // A reply to one's own message goes back to its recipients.
            let mut to = without(first.clone(), &own, &mut seen);
            if to.is_empty() {
                to = without(
                    m.to.iter().map(AddressView::from).collect(),
                    &own,
                    &mut seen,
                );
            }
            (to, Vec::new())
        };
        let synthetic = m.message_id.starts_with("synth:");
        let mut references: Vec<String> = match self.store.message_refs(message_id).await? {
            Some(refs) => refs.split_whitespace().map(str::to_string).collect(),
            None => Vec::new(),
        };
        if !synthetic {
            references.push(m.message_id.clone());
        }
        // The identity the original was addressed to, else the default.
        let identities = self.store.list_identities(m.account_id).await?;
        let addressed: Vec<String> =
            m.to.iter()
                .chain(&m.cc)
                .map(|a| a.addr.to_ascii_lowercase())
                .collect();
        let identity_id = identities
            .iter()
            .find(|i| addressed.contains(&i.identity.address.to_ascii_lowercase()))
            .or(identities.first())
            .map(|i| i.id);
        self.new_draft(
            m.account_id,
            DraftContent {
                identity_id,
                to,
                cc,
                subject: prefixed("Re", &m.subject),
                in_reply_to: (!synthetic).then(|| m.message_id.clone()),
                references,
                original: Some(message_id),
                mode: if all { "reply_all" } else { "reply" }.into(),
                quote: Some(Quote {
                    date_utc: m.date_utc,
                    from: AddressView::from(&m.from),
                    to: m.to.iter().map(AddressView::from).collect(),
                    subject: m.subject.clone(),
                    text: body.map(|b| b.text_plain).unwrap_or_default(),
                    forwarded: false,
                }),
                ..DraftContent::default()
            },
        )
        .await
    }

    /// `compose.forward`: inline by default, or as a `message/rfc822` attachment.
    pub async fn compose_forward(
        &self,
        message_id: MessageId,
        as_attachment: bool,
    ) -> Result<DraftView, AppError> {
        let m = self.store.get_message(message_id).await?;
        let body = self.store.get_body(message_id).await?;
        let attachments = if as_attachment {
            Vec::new()
        } else {
            // The original's files travel with an inline forward: fetched now, on demand.
            let mut files = Vec::new();
            for a in self.store.list_attachments(message_id).await? {
                if a.attachment.is_inline {
                    continue;
                }
                files.push(a);
            }
            files
        };
        let view = self
            .new_draft(
                m.account_id,
                DraftContent {
                    subject: prefixed("Fwd", &m.subject),
                    original: Some(message_id),
                    forward_attachment: as_attachment.then_some(message_id),
                    mode: "forward".into(),
                    quote: (!as_attachment).then(|| Quote {
                        date_utc: m.date_utc,
                        from: AddressView::from(&m.from),
                        to: m.to.iter().map(AddressView::from).collect(),
                        subject: m.subject.clone(),
                        text: body.map(|b| b.text_plain).unwrap_or_default(),
                        forwarded: true,
                    }),
                    ..DraftContent::default()
                },
            )
            .await?;
        for a in attachments {
            let name = mail_render::display_name(&a.attachment.filename);
            match self.attachment_bytes(&a).await {
                Ok(bytes) => {
                    self.attach_bytes(view.draft_id, &name, &a.attachment.mime, &bytes)
                        .await?;
                }
                Err(e) => {
                    tracing::warn!(code = ?e.code, "a forwarded attachment could not be fetched")
                }
            }
        }
        self.draft_view(view.draft_id).await
    }

    /// `draft.save`: the composer's autosave; the upload follows 30 s after the last change.
    pub async fn draft_save(
        &self,
        draft_id: DraftId,
        content: DraftContent,
    ) -> Result<(), AppError> {
        let value = serde_json::to_value(&content)
            .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?;
        self.store
            .update_draft(draft_id, value, crate::accounts::now())
            .await?;
        self.schedule_upload(draft_id);
        Ok(())
    }

    fn schedule_upload(&self, draft_id: DraftId) {
        let me = self.me.get().cloned();
        let delay = self.timing().draft_upload;
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Some(core) = me.and_then(|w: Weak<Self>| w.upgrade()) {
                if let Err(e) = core.upload_draft(draft_id).await {
                    tracing::warn!(code = ?e.code, "draft upload not queued");
                }
            }
        });
        if let Some(old) = crate::core::lock(&self.upload_timers).insert(draft_id, task) {
            old.abort();
        }
    }

    /// The compose window closed: upload now (section 7), unless it was sent or discarded.
    pub async fn draft_close(&self, draft_id: DraftId) -> Result<(), AppError> {
        if let Some(t) = crate::core::lock(&self.upload_timers).remove(&draft_id) {
            t.abort();
        }
        match self.store.get_draft(draft_id).await {
            Ok(d) => {
                let sending = self.store.list_outbox(d.account_id).await?.iter().any(|o| {
                    o.envelope["draft_id"].as_i64() == Some(draft_id)
                        && o.state != OutboxState::Sent
                });
                if sending {
                    return Ok(());
                }
                self.upload_draft(draft_id).await
            }
            Err(StoreError::NotFound) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Builds the draft and queues its upload: APPEND the new version with `\Draft`, then
    /// remove the previous UID (section 7; G66).
    pub async fn upload_draft(&self, draft_id: DraftId) -> Result<(), AppError> {
        let draft = match self.store.get_draft(draft_id).await {
            Ok(d) => d,
            Err(StoreError::NotFound) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let folders = self.store.list_folders(draft.account_id).await?;
        let Some(drafts) = folders
            .iter()
            .find(|f| f.special_use == Some(SpecialUse::Drafts))
        else {
            return Ok(());
        };
        let content: DraftContent = serde_json::from_value(draft.content).unwrap_or_default();
        let built = self
            .build(draft.account_id, draft_id, &content, true)
            .await?;
        let op = ops::draft_replace_op(
            draft.account_id,
            drafts,
            draft_id,
            base64(&built.raw),
            &built.message_id,
        );
        self.queue_ops(vec![op]).await.map(|_| ())
    }

    /// `draft.discard`: the local draft, its files and its uploaded copy go.
    pub async fn draft_discard(&self, draft_id: DraftId) -> Result<(), AppError> {
        if let Some(t) = crate::core::lock(&self.upload_timers).remove(&draft_id) {
            t.abort();
        }
        let draft = match self.store.get_draft(draft_id).await {
            Ok(d) => d,
            Err(StoreError::NotFound) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        self.remove_draft(draft.account_id, draft_id, draft.imap_uid)
            .await
    }

    async fn remove_draft(
        &self,
        account_id: AccountId,
        draft_id: DraftId,
        uid: Option<u32>,
    ) -> Result<(), AppError> {
        self.store.delete_draft(draft_id).await?;
        let dir = self.drafts_dir(draft_id);
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await;
        if let Some(uid) = uid {
            let folders = self.store.list_folders(account_id).await?;
            if let Some(drafts) = folders
                .iter()
                .find(|f| f.special_use == Some(SpecialUse::Drafts))
            {
                self.queue_ops(vec![ops::draft_delete_op(account_id, drafts, uid)])
                    .await?;
            }
        }
        Ok(())
    }

    async fn attach_bytes(
        &self,
        draft_id: DraftId,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<DraftContent, AppError> {
        let draft = self.store.get_draft(draft_id).await?;
        let mut content: DraftContent = serde_json::from_value(draft.content).unwrap_or_default();
        let id = uuid_like();
        let dir = self.drafts_dir(draft_id);
        let path = dir.join(&id);
        let data = bytes.to_vec();
        tokio::task::spawn_blocking(move || {
            mail_render::files::create_private_dir(&dir)?;
            std::fs::write(&path, data)
        })
        .await
        .map_err(|_| AppError::new(ErrorCode::Internal, "file task failed"))?
        .map_err(|e| AppError::new(ErrorCode::Io, e.kind().to_string()))?;
        content.attachments.push(DraftAttachment {
            id,
            name: mail_render::safe_name(name),
            mime: mime.to_string(),
            size: bytes.len() as u64,
        });
        self.draft_save(draft_id, content.clone()).await?;
        Ok(content)
    }

    /// `draft.add_attachment`: the file is copied into the app data directory now, so a moved
    /// or deleted original cannot break a later send (section 7).
    pub async fn draft_add_attachment(
        &self,
        draft_id: DraftId,
        path: PathBuf,
    ) -> Result<DraftContent, AppError> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| mail_render::files::FALLBACK_NAME.to_string());
        let source = path.clone();
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(source))
            .await
            .map_err(|_| AppError::new(ErrorCode::Internal, "file task failed"))?
            .map_err(|e| AppError::new(ErrorCode::Io, e.kind().to_string()))?;
        let mime = mime_for(&name);
        self.attach_bytes(draft_id, &name, mime, &bytes).await
    }

    /// `draft.remove_attachment`.
    pub async fn draft_remove_attachment(
        &self,
        draft_id: DraftId,
        attachment_id: &str,
    ) -> Result<DraftContent, AppError> {
        let draft = self.store.get_draft(draft_id).await?;
        let mut content: DraftContent = serde_json::from_value(draft.content).unwrap_or_default();
        content.attachments.retain(|a| a.id != attachment_id);
        if !attachment_id.is_empty()
            && attachment_id
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '-')
        {
            let path = self.drafts_dir(draft_id).join(attachment_id);
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await;
        }
        self.draft_save(draft_id, content.clone()).await?;
        Ok(content)
    }

    /// The message for a draft: the identity's address as From (section 7).
    async fn build(
        &self,
        account_id: AccountId,
        draft_id: DraftId,
        content: &DraftContent,
        as_draft: bool,
    ) -> Result<mail_smtp::BuiltMessage, AppError> {
        let identities = self.store.list_identities(account_id).await?;
        let identity = content
            .identity_id
            .and_then(|id| identities.iter().find(|i| i.id == id))
            .or(identities.first())
            .ok_or_else(|| bad("the account has no identity"))?;
        let mut files = Vec::new();
        for a in &content.attachments {
            let path = self.drafts_dir(draft_id).join(&a.id);
            let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
                .await
                .map_err(|_| AppError::new(ErrorCode::Internal, "file task failed"))?
                .map_err(|e| AppError::new(ErrorCode::Io, e.kind().to_string()))?;
            files.push(OutgoingAttachment {
                filename: a.name.clone(),
                mime: a.mime.clone(),
                bytes,
            });
        }
        let forwarded = match (content.forward_attachment, as_draft) {
            (Some(id), false) => {
                let m = self.store.get_message(id).await?;
                Some(self.fetch_source(m.account_id, id).await?)
            }
            _ => None,
        };
        let from = MailboxSpec {
            name: identity.identity.name.clone(),
            addr: identity.identity.address.clone(),
        };
        let mut to: Vec<MailboxSpec> = content.to.iter().map(spec).collect();
        if as_draft && to.is_empty() && content.cc.is_empty() && content.bcc.is_empty() {
            // A draft may have no recipient yet; the envelope is never used for it.
            to.push(from.clone());
        }
        let message = OutgoingMessage {
            from: Some(from),
            to,
            cc: content.cc.iter().map(spec).collect(),
            bcc: content.bcc.iter().map(spec).collect(),
            reply_to: identity.identity.reply_to.clone(),
            subject: content.subject.clone(),
            text: content.text.clone(),
            html: (!content.plain_only && !content.html.trim().is_empty())
                .then(|| mail_render::sanitize_outgoing(&content.html)),
            attachments: files,
            forwarded,
            in_reply_to: content.in_reply_to.clone(),
            references: content.references.clone(),
            message_id: Some(content.message_id.clone()).filter(|m| !m.is_empty()),
            date: crate::accounts::now(),
        };
        message.build().map_err(|e| bad(&e.to_string()))
    }

    async fn fetch_source(
        &self,
        account_id: AccountId,
        message_id: MessageId,
    ) -> Result<Vec<u8>, AppError> {
        self.fetch_part_pub(account_id, message_id, "").await
    }

    /// `draft.send`: checks, builds, and queues with the undo window (or at `send_at`).
    pub async fn draft_send(
        &self,
        draft_id: DraftId,
        identity_id: IdentityId,
        send_at: Option<i64>,
    ) -> Result<OutboxId, AppError> {
        let draft = self.store.get_draft(draft_id).await?;
        let identity = self.store.get_identity(identity_id).await?;
        let mut content: DraftContent = serde_json::from_value(draft.content).unwrap_or_default();
        content.identity_id = Some(identity_id);
        let account_id = identity.account_id;
        if content.to.is_empty() && content.cc.is_empty() && content.bcc.is_empty() {
            return Err(bad("add a recipient"));
        }
        let built = self.build(account_id, draft_id, &content, false).await?;
        // Size: the SMTP SIZE limit when the server answers, and the IMAP APPENDLIMIT the
        // actor last saw (section 7; SPEC_GAPS G65).
        let account = self.store.get_account(account_id).await?;
        let size = built.raw.len() as u64;
        if let Ok(Some(limit)) = self.smtp.size_limit(&smtp_config(&account)).await {
            if size > limit {
                return Err(AppError::new(
                    ErrorCode::TooLarge,
                    format!("{size} bytes; the server accepts at most {limit}"),
                ));
            }
        }
        let append_limit = self
            .actors
            .lock()
            .await
            .get(&account_id)
            .and_then(|h| h.capabilities())
            .and_then(|c| c.append_limit.flatten());
        if let Some(limit) = append_limit {
            if size > limit {
                return Err(AppError::new(
                    ErrorCode::TooLarge,
                    format!("{size} bytes; the Sent folder accepts at most {limit}"),
                ));
            }
        }
        let value = serde_json::to_value(&content)
            .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?;
        let now = crate::accounts::now();
        self.store.update_draft(draft_id, value, now).await?;
        let window = i64::try_from(self.timing().undo_window.as_secs()).unwrap_or(10);
        let mut recipients: Vec<Value> = Vec::new();
        for a in content.to.iter().chain(&content.cc).chain(&content.bcc) {
            recipients.push(json!([a.addr, a.name]));
        }
        let envelope = json!({
            "envelope": { "from": built.envelope.from, "to": built.envelope.to },
            "draft_id": draft_id,
            "subject": content.subject,
            "to_display": content.to.iter().chain(&content.cc).map(|a| a.addr.clone()).collect::<Vec<_>>(),
            "recipients": recipients,
            // Bcc is never in a header; the outbox history keeps it for `bcc:` search (G23).
            "bcc": content.bcc.iter().map(|a| a.addr.clone()).collect::<Vec<_>>(),
            "original": content.original,
            "mode": content.mode,
            "message_id": built.message_id,
        });
        let id = self
            .store
            .insert_outbox(NewOutbox {
                account_id,
                identity_id,
                raw_mime: built.raw,
                envelope,
                send_after_utc: send_at.unwrap_or(now + window),
                created_at: now,
            })
            .await?;
        if let Some(t) = crate::core::lock(&self.upload_timers).remove(&draft_id) {
            t.abort();
        }
        (self.sink)(CoreEvent::OutboxState {
            outbox_id: id,
            account_id,
            state: "queued",
        });
        self.emit_pending().await;
        self.outbox_wake.notify_one();
        Ok(id)
    }

    /// `outbox.undo`: takes a queued, failed or `unknown` item back into its draft (section 7:
    /// Undo restores the draft; the Outbox view's Edit does the same, SPEC_GAPS G67).
    /// Returns the draft to reopen.
    pub async fn outbox_undo(&self, outbox_id: OutboxId) -> Result<Option<DraftId>, AppError> {
        let item = self.store.get_outbox(outbox_id).await?;
        let draft = item.envelope["draft_id"].as_i64();
        let taken = match item.state {
            OutboxState::Queued => self.store.undo_outbox(outbox_id).await?,
            OutboxState::Failed | OutboxState::Unknown => {
                self.store.delete_outbox(outbox_id).await?;
                true
            }
            _ => false,
        };
        if !taken {
            return Err(AppError::new(
                ErrorCode::Conflict,
                "the message is already being sent",
            ));
        }
        (self.sink)(CoreEvent::OutboxState {
            outbox_id,
            account_id: item.account_id,
            state: "undone",
        });
        self.emit_pending().await;
        Ok(draft)
    }

    /// `outbox.retry`: a failed or `unknown` item is queued now.
    pub async fn outbox_retry(&self, outbox_id: OutboxId) -> Result<(), AppError> {
        let item = self.store.get_outbox(outbox_id).await?;
        if !self
            .store
            .retry_outbox(outbox_id, crate::accounts::now())
            .await?
        {
            return Err(AppError::new(
                ErrorCode::Conflict,
                "only failed or unknown items are retried",
            ));
        }
        (self.sink)(CoreEvent::OutboxState {
            outbox_id,
            account_id: item.account_id,
            state: "queued",
        });
        self.emit_pending().await;
        self.outbox_wake.notify_one();
        Ok(())
    }

    /// `outbox.discard`: the item and its draft go (a sent item's history row too).
    pub async fn outbox_discard(&self, outbox_id: OutboxId) -> Result<(), AppError> {
        let item = self.store.get_outbox(outbox_id).await?;
        if item.state == OutboxState::Sending {
            return Err(AppError::new(
                ErrorCode::Conflict,
                "the message is being sent",
            ));
        }
        self.store.delete_outbox(outbox_id).await?;
        if let Some(draft_id) = item.envelope["draft_id"].as_i64() {
            if let Ok(d) = self.store.get_draft(draft_id).await {
                if item.state != OutboxState::Sent {
                    self.remove_draft(d.account_id, draft_id, d.imap_uid)
                        .await?;
                }
            }
        }
        (self.sink)(CoreEvent::OutboxState {
            outbox_id,
            account_id: item.account_id,
            state: "discarded",
        });
        self.emit_pending().await;
        Ok(())
    }

    /// `outbox.list` (SPEC_GAPS G69).
    pub async fn outbox_list(&self, account_id: AccountId) -> Result<Vec<OutboxView>, AppError> {
        let late_after = i64::try_from(self.timing().late_after.as_secs()).unwrap_or(60);
        Ok(self
            .store
            .list_outbox(account_id)
            .await?
            .into_iter()
            .map(|o| OutboxView {
                id: o.id,
                account_id: o.account_id,
                subject: o.envelope["subject"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                to: o.envelope["to_display"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                state: state_name(o.state),
                error: serde_json::from_str(&o.last_error).ok(),
                send_after_utc: o.send_after_utc,
                created_at: o.created_at,
                draft_id: o.envelope["draft_id"].as_i64(),
                // Sent late (section 18.3): handed to SMTP well after its time, as when the
                // app was closed when it came due.
                late: o.state == OutboxState::Sent
                    && o.envelope["sent_at"]
                        .as_i64()
                        .is_some_and(|at| at - o.send_after_utc > late_after),
            })
            .collect())
    }

    /// Sends what is due. Run by the outbox task every tick and when woken.
    pub(crate) async fn process_outbox(&self) {
        let now = crate::accounts::now();
        let due = match self.store.due_outbox(now).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "outbox not read");
                return;
            }
        };
        for id in due {
            if let Err(e) = self.send_due(id, now).await {
                tracing::warn!(outbox_id = id, code = ?e.code, "outbox item not processed");
            }
        }
    }

    async fn send_due(&self, id: OutboxId, now: i64) -> Result<(), AppError> {
        let item = self.store.get_outbox(id).await?;
        let account = self.store.get_account(item.account_id).await?;
        use mail_auth::LoginSource as _;
        let login = self
            .deps
            .logins
            .login(&mail_sync::login_request(&account))
            .await;
        let login = login
            .map(|l| match l.method {
                mail_auth::AuthMethod::Password => SmtpLogin::Password {
                    username: l.username,
                    password: l.secret.expose().to_string(),
                },
                mail_auth::AuthMethod::OauthGoogle => SmtpLogin::XOAuth2 {
                    username: l.username,
                    token: l.secret.expose().to_string(),
                },
            })
            // XOAUTH2 tokens are refreshed first (section 7); Google out of reach is a
            // transient failure, a refused grant a permanent one.
            .map_err(|e| {
                if e.is_transient() {
                    SubmitError::Transient(mail_smtp::SmtpError::Network(e.to_string()))
                } else {
                    SubmitError::AuthFailed
                }
            });
        let config = smtp_config(&account);
        let transition =
            mail_outbox::send_one(&self.store, &self.smtp, &config, login, id, now).await?;
        let Some(Transition { item, state }) = transition else {
            return Ok(());
        };
        (self.sink)(CoreEvent::OutboxState {
            outbox_id: id,
            account_id: item.account_id,
            state: state_name(state),
        });
        if state == OutboxState::Sent {
            self.store.mark_sent_at(id, now).await?;
            self.after_sent(&item).await?;
        }
        self.emit_pending().await;
        Ok(())
    }

    /// After SMTP accepted (section 7): the Sent copy, `\Answered` or `$Forwarded` on the
    /// original, the draft removed, and the recipients counted as contacts (weight 3, G67).
    async fn after_sent(&self, item: &OutboxItem) -> Result<(), AppError> {
        let folders: Vec<Folder> = self.store.list_folders(item.account_id).await?;
        let mut new = Vec::new();
        match folders
            .iter()
            .find(|f| f.special_use == Some(SpecialUse::Sent))
        {
            Some(sent) => new.push(ops::append_op(item.account_id, sent, item.id)),
            None => self.store.clear_outbox_raw(item.id).await?,
        }
        if let Some(original) = item.envelope["original"].as_i64() {
            if let Ok(m) = self.store.get_message(original).await {
                let flag = match item.envelope["mode"].as_str() {
                    Some("forward") => Some(Flag::Keyword("$Forwarded".into())),
                    Some("reply" | "reply_all") => Some(Flag::Answered),
                    _ => None,
                };
                if let Some(flag) = flag {
                    new.push(ops::flags_op(
                        m.account_id,
                        m.folder_id,
                        vec![m.uid],
                        &[flag],
                        &[],
                    ));
                }
            }
        }
        if !new.is_empty() {
            self.queue_ops(new).await?;
        }
        if let Some(draft_id) = item.envelope["draft_id"].as_i64() {
            if let Ok(d) = self.store.get_draft(draft_id).await {
                self.remove_draft(d.account_id, draft_id, d.imap_uid)
                    .await?;
            }
        }
        let people: Vec<(String, String)> = item.envelope["recipients"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|p| {
                        (
                            p[0].as_str().unwrap_or_default().to_string(),
                            p[1].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.store
            .bump_contacts(item.account_id, people, 3, crate::accounts::now())
            .await?;
        Ok(())
    }

    /// `identity.list` (SPEC_GAPS G64).
    pub async fn identity_list(
        &self,
        account_id: AccountId,
    ) -> Result<Vec<IdentityView>, AppError> {
        Ok(self
            .store
            .list_identities(account_id)
            .await?
            .iter()
            .map(identity_view)
            .collect())
    }

    /// `identity.save`: add or edit; one default per account. Extra send-as addresses are
    /// typed in by hand (section 5).
    pub async fn identity_save(
        &self,
        identity: IdentityView,
    ) -> Result<Vec<IdentityView>, AppError> {
        let address = identity.address.trim().to_string();
        if address.parse::<mail_smtp::Address>().is_err() {
            return Err(bad("not a valid email address"));
        }
        if let Some(id) = identity.id {
            if self.store.get_identity(id).await?.account_id != identity.account_id {
                return Err(bad("identity of another account"));
            }
        }
        self.store
            .save_identity(
                identity.account_id,
                identity.id,
                NewIdentity {
                    name: identity.name.trim().to_string(),
                    address,
                    reply_to: identity.reply_to.filter(|r| !r.trim().is_empty()),
                    is_default: identity.is_default,
                },
            )
            .await?;
        self.identity_list(identity.account_id).await
    }

    /// `identity.delete`: never the last one.
    pub async fn identity_delete(
        &self,
        identity_id: IdentityId,
    ) -> Result<Vec<IdentityView>, AppError> {
        let account_id = self.store.get_identity(identity_id).await?.account_id;
        self.store.delete_identity(identity_id).await?;
        self.identity_list(account_id).await
    }

    /// `contacts.suggest`: up to 10 matches (section 10).
    pub async fn contacts_suggest(
        &self,
        prefix: &str,
        account_id: AccountId,
    ) -> Result<Vec<AddressView>, AppError> {
        Ok(self
            .store
            .suggest_contacts(account_id, prefix, 10)
            .await?
            .into_iter()
            .map(|c| AddressView {
                name: c.name,
                addr: c.address,
            })
            .collect())
    }
}

/// A random hex id (attachment files, Message-IDs).
fn uuid_like() -> String {
    mail_render::nonce()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_never_stack() {
        assert_eq!(prefixed("Re", "Lunch"), "Re: Lunch");
        assert_eq!(prefixed("Re", "RE: Lunch"), "RE: Lunch");
        assert_eq!(prefixed("Re", "re:Lunch"), "re:Lunch");
        assert_eq!(prefixed("Fwd", "Re: Lunch"), "Fwd: Re: Lunch");
        assert_eq!(prefixed("Fwd", "Fwd: Lunch"), "Fwd: Lunch");
        assert_eq!(prefixed("Re", ""), "Re: ");
    }

    #[test]
    fn own_addresses_and_duplicates_drop() {
        let own = vec!["me@x.test".to_string(), "alias@x.test".to_string()];
        let mut seen = Vec::new();
        let a = |n: &str, e: &str| AddressView {
            name: n.into(),
            addr: e.into(),
        };
        let to = without(
            vec![
                a("Bob", "bob@x.test"),
                a("Me", "ME@x.test"),
                a("Bob again", "BOB@x.test"),
            ],
            &own,
            &mut seen,
        );
        let cc = without(
            vec![
                a("", "Alias@X.test"),
                a("Carol", "carol@x.test"),
                a("", "bob@x.test"),
            ],
            &own,
            &mut seen,
        );
        assert_eq!(
            to.iter().map(|x| x.addr.as_str()).collect::<Vec<_>>(),
            vec!["bob@x.test"]
        );
        assert_eq!(
            cc.iter().map(|x| x.addr.as_str()).collect::<Vec<_>>(),
            vec!["carol@x.test"]
        );
    }

    #[test]
    fn base64_and_mime_types() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b""), "");
        assert_eq!(mime_for("Report.PDF"), "application/pdf");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }
}
