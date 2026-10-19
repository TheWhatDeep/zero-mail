//! In-memory IMAP server for tests (BUILD.md section 15).
//!
//! The fake models one server's mailboxes and honors the advertised capabilities exactly:
//! without MOVE, `uid_move` fails; without UIDPLUS, `uid_expunge` fails and no
//! COPYUID/APPENDUID is returned; without CONDSTORE there are no modseqs; with `X-GM-EXT-1`
//! copies are de-duplicated by `gm_msgid` and labels are derived from folder membership.
//!
//! Tests script it from the outside: deliver mail, change flags or expunge "from another
//! client", change UIDVALIDITY, revoke a password, and queue failures for the next call of a
//! given operation. Every call is recorded for assertions (for example, T26 checks that no
//! bare EXPUNGE was issued).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::transport::{ConnectConfig, Credentials, ImapConnector, ImapTransport};
use crate::types::{
    AppendUid, BodyPart, Capabilities, CopyUid, FetchedHeader, Flag, FlagSync, FlagUpdate,
    HeaderFetch, IdleEvent, ImapDate, MailboxInfo, MailboxStatus, SearchKey, SelectInfo,
    SpecialUse, StoreOp, UidSet,
};
use crate::ImapError;

/// The operation a [`Call`] or a scripted failure refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Probe,
    Connect,
    List,
    Select,
    Search,
    FetchHeaders,
    FetchFlags,
    FetchPart,
    Store,
    Move,
    Copy,
    UidExpunge,
    Expunge,
    Append,
    Create,
    Rename,
    Delete,
    Idle,
    Noop,
    Logout,
}

/// One recorded client call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub op: Op,
    /// The selected folder, or the folder argument for folder commands and APPEND.
    pub folder: Option<String>,
    pub uids: Vec<u32>,
    /// Free-form detail: destination folder, flags, part id.
    pub arg: String,
}

/// A message as tests define it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeMessage {
    pub raw: Vec<u8>,
    pub flags: Vec<Flag>,
    /// INTERNALDATE, UTC epoch seconds.
    pub internal_date: i64,
    pub body: BodyPart,
    pub gm_msgid: Option<u64>,
    pub gm_thrid: Option<u64>,
}

impl FakeMessage {
    /// A raw RFC 5322 message, described as a single text/plain part.
    pub fn new(raw: impl Into<Vec<u8>>, internal_date: i64) -> Self {
        let raw = raw.into();
        let body_len = split_header(&raw).1.len() as u64;
        Self {
            raw,
            flags: Vec::new(),
            internal_date,
            body: BodyPart::text_plain(body_len),
            gm_msgid: None,
            gm_thrid: None,
        }
    }

    /// A minimal text/plain message.
    pub fn simple(
        message_id: &str,
        from: &str,
        to: &str,
        subject: &str,
        text: &str,
        internal_date: i64,
    ) -> Self {
        let raw = format!(
            "Message-ID: <{message_id}>\r\nFrom: {from}\r\nTo: {to}\r\nSubject: {subject}\r\n\
             MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n"
        );
        Self::new(raw, internal_date)
    }

    pub fn with_flags(mut self, flags: impl IntoIterator<Item = Flag>) -> Self {
        self.flags = flags.into_iter().collect();
        self
    }

    pub fn with_body(mut self, body: BodyPart) -> Self {
        self.body = body;
        self
    }

    /// A raw RFC 5322 message whose BODYSTRUCTURE is read from its own MIME headers, so
    /// multipart bodies, attachments and inline images can be fetched part by part.
    pub fn mime(raw: impl Into<Vec<u8>>, internal_date: i64) -> Self {
        let raw = raw.into();
        let body = structure(&raw, None);
        Self {
            body,
            ..Self::new(raw, internal_date)
        }
    }

    pub fn with_gmail_ids(mut self, msgid: u64, thrid: u64) -> Self {
        self.gm_msgid = Some(msgid);
        self.gm_thrid = Some(thrid);
        self
    }
}

#[derive(Debug, Clone)]
struct Stored {
    msg: FakeMessage,
    modseq: u64,
}

#[derive(Debug)]
struct Folder {
    uidvalidity: u32,
    uidnext: u32,
    special_use: Option<SpecialUse>,
    messages: BTreeMap<u32, Stored>,
    /// Expunged UIDs and the modseq at which they vanished (QRESYNC).
    vanished: Vec<(u32, u64)>,
    highest_modseq: u64,
    /// Bumped on every change, so IDLE wakes only for its own folder.
    version: u64,
}

impl Folder {
    fn new(uidvalidity: u32, special_use: Option<SpecialUse>) -> Self {
        Self {
            uidvalidity,
            uidnext: 1,
            special_use,
            messages: BTreeMap::new(),
            vanished: Vec::new(),
            highest_modseq: 1,
            version: 0,
        }
    }
}

#[derive(Debug)]
struct State {
    caps: Capabilities,
    users: BTreeMap<String, String>,
    folders: BTreeMap<String, Folder>,
    /// (operation, successful calls still to let through, error).
    faults: VecDeque<(Op, usize, ImapError)>,
    calls: Vec<Call>,
    modseq: u64,
    next_uidvalidity: u32,
    next_gm_msgid: u64,
}

impl State {
    fn next_modseq(&mut self) -> u64 {
        self.modseq += 1;
        self.modseq
    }

    fn folder(&self, path: &str) -> Result<&Folder, ImapError> {
        self.folders.get(path).ok_or(ImapError::NoSuchMailbox)
    }

    fn folder_mut(&mut self, path: &str) -> Result<&mut Folder, ImapError> {
        self.folders.get_mut(path).ok_or(ImapError::NoSuchMailbox)
    }

    /// Inserts a message, de-duplicating by `gm_msgid` on Gmail. Returns the UID in `path`.
    fn insert(&mut self, path: &str, mut msg: FakeMessage) -> Result<u32, ImapError> {
        if self.caps.gmail {
            if msg.gm_msgid.is_none() {
                self.next_gm_msgid += 1;
                msg.gm_msgid = Some(self.next_gm_msgid);
                msg.gm_thrid = msg.gm_thrid.or(msg.gm_msgid);
            }
            let existing = self
                .folder(path)?
                .messages
                .iter()
                .find_map(|(uid, s)| (s.msg.gm_msgid == msg.gm_msgid).then_some(*uid));
            if let Some(uid) = existing {
                return Ok(uid);
            }
        }
        let modseq = self.next_modseq();
        let folder = self.folder_mut(path)?;
        let uid = folder.uidnext;
        folder.uidnext += 1;
        folder.messages.insert(uid, Stored { msg, modseq });
        folder.highest_modseq = modseq;
        folder.version += 1;
        Ok(uid)
    }

    fn remove(&mut self, path: &str, uids: &[u32]) -> Result<(), ImapError> {
        if uids.is_empty() {
            return Ok(());
        }
        let modseq = self.next_modseq();
        let folder = self.folder_mut(path)?;
        for uid in uids {
            if folder.messages.remove(uid).is_some() {
                folder.vanished.push((*uid, modseq));
            }
        }
        folder.highest_modseq = modseq;
        folder.version += 1;
        Ok(())
    }

    fn gm_labels(&self, gm_msgid: Option<u64>) -> Vec<String> {
        let Some(id) = gm_msgid else {
            return Vec::new();
        };
        self.folders
            .iter()
            .filter(|(_, f)| f.messages.values().any(|s| s.msg.gm_msgid == Some(id)))
            .filter_map(|(path, f)| match f.special_use {
                Some(SpecialUse::All) => None,
                Some(SpecialUse::Inbox) => Some("\\Inbox".to_string()),
                Some(SpecialUse::Sent) => Some("\\Sent".to_string()),
                Some(SpecialUse::Drafts) => Some("\\Draft".to_string()),
                Some(SpecialUse::Trash) => Some("\\Trash".to_string()),
                Some(SpecialUse::Junk) => Some("\\Spam".to_string()),
                Some(SpecialUse::Archive) | None => Some(path.clone()),
            })
            .collect()
    }
}

/// The fake server. Cheap to clone; clones share state.
#[derive(Debug, Clone)]
pub struct FakeServer {
    state: Arc<Mutex<State>>,
    changes: Arc<watch::Sender<u64>>,
}

impl FakeServer {
    /// A server advertising exactly `capabilities`, with an empty INBOX.
    pub fn new(capabilities: &[&str]) -> Self {
        let mut folders = BTreeMap::new();
        folders.insert("INBOX".to_string(), Folder::new(1, Some(SpecialUse::Inbox)));
        let state = State {
            caps: Capabilities::parse(capabilities.iter().copied()),
            users: BTreeMap::new(),
            folders,
            faults: VecDeque::new(),
            calls: Vec::new(),
            modseq: 1,
            next_uidvalidity: 2,
            next_gm_msgid: 1_000,
        };
        let (tx, _rx) = watch::channel(0);
        Self {
            state: Arc::new(Mutex::new(state)),
            changes: Arc::new(tx),
        }
    }

    /// The capability set Stalwart advertises, for tests that want a full-featured server.
    pub fn full_featured() -> Self {
        Self::new(&[
            "IMAP4rev1",
            "IDLE",
            "MOVE",
            "UIDPLUS",
            "CONDSTORE",
            "QRESYNC",
            "SPECIAL-USE",
            "LIST-STATUS",
            "APPENDLIMIT=52428800",
            "AUTH=PLAIN",
        ])
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn notify(&self) {
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }

    pub fn capabilities(&self) -> Capabilities {
        self.lock().caps.clone()
    }

    pub fn add_user(&self, username: &str, password: &str) {
        self.lock()
            .users
            .insert(username.to_string(), password.to_string());
    }

    /// Changes or revokes a password (T09). Existing connections stay open, as on a real
    /// server; the next login fails.
    pub fn set_password(&self, username: &str, password: &str) {
        self.add_user(username, password);
    }

    pub fn add_folder(&self, path: &str, special_use: Option<SpecialUse>) {
        let mut s = self.lock();
        let validity = s.next_uidvalidity;
        s.next_uidvalidity += 1;
        s.folders
            .entry(path.to_string())
            .or_insert_with(|| Folder::new(validity, special_use));
    }

    /// Delivers a message, as if by SMTP or another client. Returns its UID.
    pub fn deliver(&self, path: &str, msg: FakeMessage) -> Result<u32, ImapError> {
        let uid = self.lock().insert(path, msg)?;
        self.notify();
        Ok(uid)
    }

    /// Replaces a message's flags, as another client would.
    pub fn set_flags(&self, path: &str, uid: u32, flags: Vec<Flag>) -> Result<(), ImapError> {
        {
            let mut s = self.lock();
            let modseq = s.next_modseq();
            let folder = s.folder_mut(path)?;
            let stored = folder
                .messages
                .get_mut(&uid)
                .ok_or_else(|| ImapError::No("no such uid".into()))?;
            stored.msg.flags = flags;
            stored.modseq = modseq;
            folder.highest_modseq = modseq;
            folder.version += 1;
        }
        self.notify();
        Ok(())
    }

    /// Expunges a message, as another client would.
    pub fn expunge_external(&self, path: &str, uid: u32) -> Result<(), ImapError> {
        self.lock().remove(path, &[uid])?;
        self.notify();
        Ok(())
    }

    /// Sets a new UIDVALIDITY and renumbers every message from 1, as a server does after
    /// rebuilding a mailbox (T02).
    pub fn change_uidvalidity(&self, path: &str, uidvalidity: u32) -> Result<(), ImapError> {
        {
            let mut s = self.lock();
            let modseq = s.next_modseq();
            let folder = s.folder_mut(path)?;
            let old = std::mem::take(&mut folder.messages);
            folder.uidvalidity = uidvalidity;
            folder.vanished.clear();
            folder.uidnext = 1;
            for (_, mut stored) in old {
                stored.modseq = modseq;
                folder.messages.insert(folder.uidnext, stored);
                folder.uidnext += 1;
            }
            folder.highest_modseq = modseq;
            folder.version += 1;
        }
        self.notify();
        Ok(())
    }

    /// The next call of `op` fails with `error`. Failures queue in order.
    pub fn fail_next(&self, op: Op, error: ImapError) {
        self.fail_after(op, 0, error);
    }

    /// The call of `op` after the next `succeed` successful ones fails with `error`; for
    /// example, the second header batch of a sync.
    pub fn fail_after(&self, op: Op, succeed: usize, error: ImapError) {
        self.lock().faults.push_back((op, succeed, error));
    }

    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    pub fn clear_calls(&self) {
        self.lock().calls.clear();
    }

    pub fn folder_paths(&self) -> Vec<String> {
        self.lock().folders.keys().cloned().collect()
    }

    pub fn uids(&self, path: &str) -> Vec<u32> {
        self.lock()
            .folders
            .get(path)
            .map(|f| f.messages.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn flags(&self, path: &str, uid: u32) -> Option<Vec<Flag>> {
        self.lock()
            .folders
            .get(path)?
            .messages
            .get(&uid)
            .map(|s| s.msg.flags.clone())
    }

    /// Records a call and returns a queued failure for `op`, if any.
    fn begin(
        &self,
        op: Op,
        folder: Option<&str>,
        uids: &UidSet,
        arg: &str,
    ) -> Result<(), ImapError> {
        let mut s = self.lock();
        s.calls.push(Call {
            op,
            folder: folder.map(str::to_string),
            uids: uids.iter().collect(),
            arg: arg.to_string(),
        });
        if let Some(pos) = s.faults.iter().position(|(o, _, _)| *o == op) {
            if let Some(fault) = s.faults.get_mut(pos) {
                if fault.1 > 0 {
                    fault.1 -= 1;
                    return Ok(());
                }
            }
            if let Some((_, _, err)) = s.faults.remove(pos) {
                return Err(err);
            }
        }
        Ok(())
    }
}

impl ImapConnector for FakeServer {
    type Transport = FakeTransport;

    async fn probe(&self, _config: &ConnectConfig) -> Result<Capabilities, ImapError> {
        self.begin(Op::Probe, None, &UidSet::default(), "")?;
        Ok(self.lock().caps.clone())
    }

    async fn connect(
        &self,
        _config: &ConnectConfig,
        credentials: &Credentials,
    ) -> Result<FakeTransport, ImapError> {
        self.begin(Op::Connect, None, &UidSet::default(), "")?;
        let (user, secret) = match credentials {
            Credentials::Password { username, password } => (username, password),
            Credentials::XOAuth2 {
                username,
                access_token,
            } => (username, access_token),
        };
        let caps = {
            let s = self.lock();
            if s.users.get(user).map(String::as_str) != Some(secret.expose()) {
                return Err(ImapError::AuthFailed);
            }
            s.caps.clone()
        };
        Ok(FakeTransport {
            server: self.clone(),
            caps,
            selected: None,
            rx: self.changes.subscribe(),
        })
    }
}

/// One fake connection.
#[derive(Debug)]
pub struct FakeTransport {
    server: FakeServer,
    caps: Capabilities,
    selected: Option<String>,
    rx: watch::Receiver<u64>,
}

impl FakeTransport {
    fn selected(&self) -> Result<String, ImapError> {
        self.selected.clone().ok_or(ImapError::NotSelected)
    }

    fn begin_selected(&self, op: Op, uids: &UidSet, arg: &str) -> Result<String, ImapError> {
        let path = self.selected()?;
        self.server.begin(op, Some(&path), uids, arg)?;
        Ok(path)
    }

    fn do_list(&self) -> Result<Vec<MailboxInfo>, ImapError> {
        self.server.begin(Op::List, None, &UidSet::default(), "")?;
        let s = self.server.lock();
        Ok(s.folders
            .iter()
            .map(|(path, f)| {
                let role = match f.special_use {
                    Some(SpecialUse::Inbox) => Some(SpecialUse::Inbox),
                    other if self.caps.special_use => other,
                    _ => None,
                };
                let attributes = role
                    .filter(|r| *r != SpecialUse::Inbox)
                    .map(|r| vec![r.as_attribute().to_string()])
                    .unwrap_or_default();
                let status = self.caps.list_status.then(|| MailboxStatus {
                    messages: f.messages.len() as u32,
                    unseen: f
                        .messages
                        .values()
                        .filter(|m| !m.msg.flags.contains(&Flag::Seen))
                        .count() as u32,
                    uidnext: f.uidnext,
                    uidvalidity: f.uidvalidity,
                });
                MailboxInfo {
                    path: path.clone(),
                    delimiter: Some('/'),
                    attributes,
                    special_use: role,
                    status,
                }
            })
            .collect())
    }

    fn do_select(&mut self, path: &str) -> Result<SelectInfo, ImapError> {
        self.server
            .begin(Op::Select, Some(path), &UidSet::default(), "")?;
        let info = {
            let s = self.server.lock();
            let f = s.folder(path)?;
            SelectInfo {
                uidvalidity: f.uidvalidity,
                uidnext: f.uidnext,
                exists: f.messages.len() as u32,
                highest_modseq: self.caps.condstore.then_some(f.highest_modseq),
                permanent_flags: vec![
                    "\\Seen".into(),
                    "\\Flagged".into(),
                    "\\Answered".into(),
                    "\\Draft".into(),
                    "\\Deleted".into(),
                    "\\*".into(),
                ],
            }
        };
        self.selected = Some(path.to_string());
        Ok(info)
    }

    fn do_search(&self, key: &SearchKey) -> Result<Vec<u32>, ImapError> {
        let path = self.begin_selected(Op::Search, &UidSet::default(), &format!("{key:?}"))?;
        if contains_gmail_raw(key) && !self.caps.gmail {
            return Err(ImapError::Unsupported("X-GM-EXT-1"));
        }
        let s = self.server.lock();
        let f = s.folder(&path)?;
        let mut found: Vec<u32> = f
            .messages
            .iter()
            .filter(|(uid, m)| matches(key, **uid, &m.msg))
            .map(|(uid, _)| *uid)
            .collect();
        // RFC 3501: `n:*` always includes the highest UID, even when it is below n.
        if let (SearchKey::UidFrom(_), true) = (key, found.is_empty()) {
            found.extend(f.messages.keys().next_back().copied());
        }
        Ok(found)
    }

    fn do_fetch_headers(
        &self,
        uids: &UidSet,
        options: HeaderFetch,
    ) -> Result<Vec<FetchedHeader>, ImapError> {
        let path = self.begin_selected(Op::FetchHeaders, uids, "")?;
        let s = self.server.lock();
        let f = s.folder(&path)?;
        let gmail = options.gmail && self.caps.gmail;
        Ok(uids
            .iter()
            .filter_map(|uid| f.messages.get(&uid).map(|m| (uid, m)))
            .map(|(uid, m)| FetchedHeader {
                uid,
                flags: m.msg.flags.clone(),
                internal_date: m.msg.internal_date,
                size: m.msg.raw.len() as u32,
                headers: split_header(&m.msg.raw).0.to_vec(),
                body: m.msg.body.clone(),
                modseq: self.caps.condstore.then_some(m.modseq),
                gm_msgid: if gmail { m.msg.gm_msgid } else { None },
                gm_thrid: if gmail { m.msg.gm_thrid } else { None },
                gm_labels: if gmail {
                    s.gm_labels(m.msg.gm_msgid)
                } else {
                    Vec::new()
                },
            })
            .collect())
    }

    fn do_fetch_flags(
        &self,
        uids: &UidSet,
        changed_since: Option<u64>,
    ) -> Result<FlagSync, ImapError> {
        let path = self.begin_selected(Op::FetchFlags, uids, &format!("{changed_since:?}"))?;
        let s = self.server.lock();
        let f = s.folder(&path)?;
        let since = if self.caps.condstore {
            changed_since
        } else {
            None
        };
        let updates = uids
            .iter()
            .filter_map(|uid| f.messages.get(&uid).map(|m| (uid, m)))
            .filter(|(_, m)| since.is_none_or(|ms| m.modseq > ms))
            .map(|(uid, m)| FlagUpdate {
                uid,
                flags: m.msg.flags.clone(),
                modseq: self.caps.condstore.then_some(m.modseq),
            })
            .collect();
        let vanished = match (self.caps.qresync, since) {
            (true, Some(ms)) => f
                .vanished
                .iter()
                .filter(|(uid, at)| *at > ms && uids.contains(*uid))
                .map(|(uid, _)| *uid)
                .collect(),
            _ => Vec::new(),
        };
        Ok(FlagSync { updates, vanished })
    }

    fn do_fetch_part(
        &self,
        uid: u32,
        part: &str,
        max_bytes: Option<u64>,
    ) -> Result<Vec<u8>, ImapError> {
        let path = self.begin_selected(Op::FetchPart, &UidSet::single(uid), part)?;
        let s = self.server.lock();
        let m = s
            .folder(&path)?
            .messages
            .get(&uid)
            .ok_or_else(|| ImapError::No("no such uid".into()))?;
        let (head, body) = split_header(&m.msg.raw);
        let bytes = match part {
            "" => m.msg.raw.as_slice(),
            "HEADER" => head,
            "TEXT" => body,
            "1" if m.msg.body.children.is_empty() => body,
            id => part_bytes(&m.msg.raw, id).ok_or_else(|| ImapError::No("no such part".into()))?,
        };
        let limit = max_bytes.map_or(bytes.len(), |max| bytes.len().min(max as usize));
        Ok(bytes[..limit].to_vec())
    }

    fn do_store(&self, uids: &UidSet, op: StoreOp, flags: &[Flag]) -> Result<(), ImapError> {
        let arg = format!(
            "{op:?} {}",
            flags
                .iter()
                .map(Flag::as_imap)
                .collect::<Vec<_>>()
                .join(" ")
        );
        let path = self.begin_selected(Op::Store, uids, &arg)?;
        {
            let mut s = self.server.lock();
            let modseq = s.next_modseq();
            let f = s.folder_mut(&path)?;
            for uid in uids.iter() {
                if let Some(m) = f.messages.get_mut(&uid) {
                    for flag in flags {
                        let has = m.msg.flags.contains(flag);
                        match op {
                            StoreOp::Add if !has => m.msg.flags.push(flag.clone()),
                            StoreOp::Remove if has => m.msg.flags.retain(|x| x != flag),
                            _ => {}
                        }
                    }
                    m.modseq = modseq;
                }
            }
            f.highest_modseq = modseq;
            f.version += 1;
        }
        self.server.notify();
        Ok(())
    }

    fn do_copy(&self, op: Op, uids: &UidSet, dest: &str) -> Result<Option<CopyUid>, ImapError> {
        let path = self.begin_selected(op, uids, dest)?;
        if op == Op::Move && !self.caps.move_ {
            return Err(ImapError::Unsupported("MOVE"));
        }
        let result = {
            let mut s = self.server.lock();
            let dest_validity = s.folder(dest)?.uidvalidity;
            let sources: Vec<(u32, FakeMessage)> = {
                let f = s.folder(&path)?;
                uids.iter()
                    .filter_map(|uid| f.messages.get(&uid).map(|m| (uid, m.msg.clone())))
                    .collect()
            };
            let mut copied = CopyUid {
                uidvalidity: dest_validity,
                source: Vec::new(),
                dest: Vec::new(),
            };
            for (uid, msg) in sources {
                let new_uid = s.insert(dest, msg)?;
                copied.source.push(uid);
                copied.dest.push(new_uid);
            }
            if op == Op::Move {
                s.remove(&path, &copied.source)?;
            }
            copied
        };
        self.server.notify();
        Ok(self.caps.uidplus.then_some(result))
    }

    fn do_expunge(&self, only: Option<&UidSet>) -> Result<(), ImapError> {
        let path = match only {
            Some(uids) => {
                let path = self.begin_selected(Op::UidExpunge, uids, "")?;
                if !self.caps.uidplus {
                    return Err(ImapError::Unsupported("UIDPLUS"));
                }
                path
            }
            None => self.begin_selected(Op::Expunge, &UidSet::default(), "")?,
        };
        {
            let mut s = self.server.lock();
            let doomed: Vec<u32> = s
                .folder(&path)?
                .messages
                .iter()
                .filter(|(uid, m)| {
                    m.msg.flags.contains(&Flag::Deleted)
                        && only.is_none_or(|set| set.contains(**uid))
                })
                .map(|(uid, _)| *uid)
                .collect();
            s.remove(&path, &doomed)?;
        }
        self.server.notify();
        Ok(())
    }

    fn do_append(
        &self,
        folder: &str,
        flags: &[Flag],
        raw: &[u8],
    ) -> Result<Option<AppendUid>, ImapError> {
        self.server
            .begin(Op::Append, Some(folder), &UidSet::default(), "")?;
        let result = {
            let mut s = self.server.lock();
            let uidvalidity = s.folder(folder)?.uidvalidity;
            let mut msg = FakeMessage::new(raw.to_vec(), 0);
            msg.flags = flags.to_vec();
            let uid = s.insert(folder, msg)?;
            AppendUid { uidvalidity, uid }
        };
        self.server.notify();
        Ok(self.caps.uidplus.then_some(result))
    }

    fn do_folder(&self, op: Op, path: &str, to: Option<&str>) -> Result<(), ImapError> {
        self.server
            .begin(op, Some(path), &UidSet::default(), to.unwrap_or(""))?;
        let mut s = self.server.lock();
        match (op, to) {
            (Op::Create, _) => {
                if s.folders.contains_key(path) {
                    return Err(ImapError::No("mailbox already exists".into()));
                }
                let validity = s.next_uidvalidity;
                s.next_uidvalidity += 1;
                s.folders
                    .insert(path.to_string(), Folder::new(validity, None));
            }
            (Op::Rename, Some(to)) => {
                if path.eq_ignore_ascii_case("INBOX") || s.folders.contains_key(to) {
                    return Err(ImapError::No("cannot rename".into()));
                }
                let folder = s.folders.remove(path).ok_or(ImapError::NoSuchMailbox)?;
                s.folders.insert(to.to_string(), folder);
                let prefix = format!("{path}/");
                let children: Vec<String> = s
                    .folders
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .cloned()
                    .collect();
                for child in children {
                    if let Some(f) = s.folders.remove(&child) {
                        s.folders
                            .insert(format!("{to}/{}", &child[prefix.len()..]), f);
                    }
                }
            }
            (Op::Delete, _) => {
                if path.eq_ignore_ascii_case("INBOX") {
                    return Err(ImapError::No("cannot delete INBOX".into()));
                }
                s.folders.remove(path).ok_or(ImapError::NoSuchMailbox)?;
            }
            _ => return Err(ImapError::Bad("unexpected folder operation".into())),
        }
        Ok(())
    }

    fn folder_version(&self, path: &str) -> Result<u64, ImapError> {
        Ok(self.server.lock().folder(path)?.version)
    }
}

impl ImapTransport for FakeTransport {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn list(&mut self) -> Result<Vec<MailboxInfo>, ImapError> {
        self.do_list()
    }

    async fn select(&mut self, path: &str) -> Result<SelectInfo, ImapError> {
        self.do_select(path)
    }

    async fn uid_search(&mut self, key: &SearchKey) -> Result<Vec<u32>, ImapError> {
        self.do_search(key)
    }

    async fn uid_fetch_headers(
        &mut self,
        uids: &UidSet,
        options: HeaderFetch,
    ) -> Result<Vec<FetchedHeader>, ImapError> {
        self.do_fetch_headers(uids, options)
    }

    async fn uid_fetch_flags(
        &mut self,
        uids: &UidSet,
        changed_since: Option<u64>,
    ) -> Result<FlagSync, ImapError> {
        self.do_fetch_flags(uids, changed_since)
    }

    async fn uid_fetch_changes(
        &mut self,
        up_to: u32,
        changed_since: u64,
    ) -> Result<FlagSync, ImapError> {
        if !self.caps.condstore {
            return Err(ImapError::Unsupported("CONDSTORE"));
        }
        let path = self.selected()?;
        let all: Vec<u32> = {
            let s = self.server.lock();
            let f = s.folder(&path)?;
            let mut uids: Vec<u32> = f.messages.keys().copied().filter(|u| *u <= up_to).collect();
            uids.extend(f.vanished.iter().map(|(u, _)| *u).filter(|u| *u <= up_to));
            uids
        };
        self.do_fetch_flags(&UidSet::new(all), Some(changed_since))
    }

    async fn uid_fetch_part(
        &mut self,
        uid: u32,
        part: &str,
        max_bytes: Option<u64>,
    ) -> Result<Vec<u8>, ImapError> {
        self.do_fetch_part(uid, part, max_bytes)
    }

    async fn uid_store(
        &mut self,
        uids: &UidSet,
        op: StoreOp,
        flags: &[Flag],
    ) -> Result<(), ImapError> {
        self.do_store(uids, op, flags)
    }

    async fn uid_move(&mut self, uids: &UidSet, dest: &str) -> Result<Option<CopyUid>, ImapError> {
        self.do_copy(Op::Move, uids, dest)
    }

    async fn uid_copy(&mut self, uids: &UidSet, dest: &str) -> Result<Option<CopyUid>, ImapError> {
        self.do_copy(Op::Copy, uids, dest)
    }

    async fn uid_expunge(&mut self, uids: &UidSet) -> Result<(), ImapError> {
        self.do_expunge(Some(uids))
    }

    async fn expunge(&mut self) -> Result<(), ImapError> {
        self.do_expunge(None)
    }

    async fn append(
        &mut self,
        folder: &str,
        flags: &[Flag],
        raw: &[u8],
    ) -> Result<Option<AppendUid>, ImapError> {
        self.do_append(folder, flags, raw)
    }

    async fn create(&mut self, path: &str) -> Result<(), ImapError> {
        self.do_folder(Op::Create, path, None)
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<(), ImapError> {
        self.do_folder(Op::Rename, from, Some(to))
    }

    async fn delete(&mut self, path: &str) -> Result<(), ImapError> {
        self.do_folder(Op::Delete, path, None)
    }

    async fn idle(&mut self, max_wait: Duration) -> Result<IdleEvent, ImapError> {
        let path = self.begin_selected(Op::Idle, &UidSet::default(), "")?;
        if !self.caps.idle {
            return Err(ImapError::Unsupported("IDLE"));
        }
        let start = self.folder_version(&path)?;
        let _ = self.rx.borrow_and_update();
        let deadline = Instant::now() + max_wait;
        loop {
            match tokio::time::timeout_at(deadline, self.rx.changed()).await {
                Err(_) => return Ok(IdleEvent::TimedOut),
                Ok(Err(_)) => return Err(ImapError::Bye),
                Ok(Ok(())) => {
                    if self.folder_version(&path)? != start {
                        return Ok(IdleEvent::Changed);
                    }
                }
            }
        }
    }

    async fn noop(&mut self) -> Result<(), ImapError> {
        self.server
            .begin(Op::Noop, self.selected.as_deref(), &UidSet::default(), "")
    }

    async fn logout(self) -> Result<(), ImapError> {
        self.server.begin(Op::Logout, None, &UidSet::default(), "")
    }
}

/// Splits a raw message at the first blank line into (header block, body).
fn split_header(raw: &[u8]) -> (&[u8], &[u8]) {
    for (i, w) in raw.windows(4).enumerate() {
        if w == b"\r\n\r\n" {
            return (&raw[..i + 2], &raw[i + 4..]);
        }
    }
    for (i, w) in raw.windows(2).enumerate() {
        if w == b"\n\n" {
            return (&raw[..i + 1], &raw[i + 2..]);
        }
    }
    (raw, &[])
}

/// The unfolded value of the first header named `name` in a header block, as written.
fn header_raw(head: &[u8], name: &str) -> Option<String> {
    let head = String::from_utf8_lossy(head);
    let mut value: Option<String> = None;
    for line in head.split("\r\n").flat_map(|l| l.split('\n')) {
        match value.as_mut() {
            Some(v) if line.starts_with([' ', '\t']) => v.push_str(line),
            Some(_) => break,
            None => {
                if let Some((key, rest)) = line.split_once(':') {
                    if key.trim().eq_ignore_ascii_case(name) {
                        value = Some(rest.trim().to_string());
                    }
                }
            }
        }
    }
    value
}

/// A header value split at `;` outside quotes: the main value, lowercased, and its
/// parameters with lowercased names and unquoted values.
fn header_params(value: &str) -> (String, Vec<(String, String)>) {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ';' if !quoted => pieces.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    pieces.push(current);
    let mut iter = pieces.into_iter();
    let main = iter.next().unwrap_or_default().trim().to_lowercase();
    let params = iter
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((
                k.trim().to_lowercase(),
                v.trim().trim_matches('"').to_string(),
            ))
        })
        .collect();
    (main, params)
}

/// A MIME entity's [`BodyPart`] tree, as a server would describe it in BODYSTRUCTURE.
/// `part_id` is `None` for the message root.
fn structure(entity: &[u8], part_id: Option<String>) -> BodyPart {
    let (head, body) = split_header(entity);
    let content_type = header_raw(head, "content-type").unwrap_or_else(|| "text/plain".into());
    let (mime, params) = header_params(&content_type);
    let param = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let (disposition, dparams) = header_raw(head, "content-disposition")
        .map(|d| header_params(&d))
        .map_or((None, Vec::new()), |(d, p)| (Some(d), p));
    let dparam = |name: &str| {
        dparams
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let filename = dparam("filename")
        .or_else(|| dparam("filename*").map(|v| crate::parse::rfc2231(&v)))
        .or_else(|| param("name"));
    let id_of = |i: usize| match &part_id {
        None => (i + 1).to_string(),
        Some(p) => format!("{p}.{}", i + 1),
    };
    if let (true, Some(boundary)) = (mime.starts_with("multipart/"), param("boundary")) {
        let children = multipart_children(body, &boundary)
            .iter()
            .enumerate()
            .map(|(i, child)| structure(child, Some(id_of(i))))
            .collect();
        return BodyPart {
            part_id: part_id.unwrap_or_default(),
            mime,
            charset: None,
            encoding: None,
            content_id: None,
            disposition: None,
            filename: None,
            size: 0,
            flowed: false,
            delsp: false,
            children,
        };
    }
    let is = |name: &str, value: &str| param(name).is_some_and(|v| v.eq_ignore_ascii_case(value));
    BodyPart {
        part_id: part_id.unwrap_or_else(|| "1".into()),
        mime,
        charset: param("charset"),
        encoding: Some(
            header_raw(head, "content-transfer-encoding")
                .unwrap_or_else(|| "7bit".into())
                .to_lowercase(),
        ),
        content_id: header_raw(head, "content-id").map(|id| {
            id.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        }),
        disposition,
        filename,
        size: body.len() as u64,
        flowed: is("format", "flowed"),
        delsp: is("delsp", "yes"),
        children: Vec::new(),
    }
}

/// The entities between `--boundary` lines, without the CRLF before each delimiter.
fn multipart_children<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delimiter = format!("--{boundary}");
    let d = delimiter.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0;
    while i + d.len() <= body.len() {
        let at_line_start = i == 0 || body[i - 1] == b'\n';
        if at_line_start && &body[i..i + d.len()] == d {
            starts.push(i);
            i += d.len();
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for w in starts.windows(2) {
        let (from, to) = (w[0], w[1]);
        let mut begin = from + d.len();
        // Skip the rest of the delimiter line.
        while begin < to && body[begin] != b'\n' {
            begin += 1;
        }
        begin = (begin + 1).min(to);
        let mut end = to;
        if end > begin && body[end - 1] == b'\n' {
            end -= 1;
        }
        if end > begin && body[end - 1] == b'\r' {
            end -= 1;
        }
        out.push(&body[begin..end]);
    }
    out
}

/// The body bytes of part `id` (for example `2` or `1.2`), still transfer-encoded.
fn part_bytes<'a>(raw: &'a [u8], id: &str) -> Option<&'a [u8]> {
    let mut entity = raw;
    let steps: Vec<&str> = id.split('.').collect();
    for (i, step) in steps.iter().enumerate() {
        let n: usize = step.parse().ok()?;
        let last = i + 1 == steps.len();
        let (head, body) = split_header(entity);
        let content_type = header_raw(head, "content-type").unwrap_or_default();
        let (mime, params) = header_params(&content_type);
        let boundary = params.iter().find(|(k, _)| k == "boundary").map(|(_, v)| v);
        match (mime.starts_with("multipart/"), boundary) {
            (true, Some(b)) => entity = multipart_children(body, b).get(n.checked_sub(1)?)?,
            _ if n == 1 && last => return Some(body),
            _ => return None,
        }
    }
    Some(split_header(entity).1)
}

/// The unfolded value of the first header named `name`, lowercased.
fn header_value(raw: &[u8], name: &str) -> String {
    let head = String::from_utf8_lossy(split_header(raw).0);
    let mut value: Option<String> = None;
    for line in head.split("\r\n").flat_map(|l| l.split('\n')) {
        match value.as_mut() {
            Some(v) if line.starts_with([' ', '\t']) => v.push_str(line),
            Some(_) => break,
            None => {
                if let Some((key, rest)) = line.split_once(':') {
                    if key.trim().eq_ignore_ascii_case(name) {
                        value = Some(rest.to_string());
                    }
                }
            }
        }
    }
    value.unwrap_or_default().to_lowercase()
}

fn contains_gmail_raw(key: &SearchKey) -> bool {
    match key {
        SearchKey::GmailRaw(_) => true,
        SearchKey::Not(k) => contains_gmail_raw(k),
        SearchKey::Or(a, b) => contains_gmail_raw(a) || contains_gmail_raw(b),
        SearchKey::And(keys) => keys.iter().any(contains_gmail_raw),
        _ => false,
    }
}

fn matches(key: &SearchKey, uid: u32, m: &FakeMessage) -> bool {
    let has = |f: &Flag| m.flags.contains(f);
    let date = ImapDate::from_epoch(m.internal_date);
    let text = || String::from_utf8_lossy(&m.raw).to_lowercase();
    match key {
        SearchKey::All => true,
        SearchKey::Since(d) => date >= *d,
        SearchKey::Before(d) => date < *d,
        SearchKey::Text(t) | SearchKey::GmailRaw(t) => text().contains(&t.to_lowercase()),
        SearchKey::From(t) => header_value(&m.raw, "from").contains(&t.to_lowercase()),
        SearchKey::To(t) => header_value(&m.raw, "to").contains(&t.to_lowercase()),
        SearchKey::Cc(t) => header_value(&m.raw, "cc").contains(&t.to_lowercase()),
        SearchKey::Bcc(t) => header_value(&m.raw, "bcc").contains(&t.to_lowercase()),
        SearchKey::Subject(t) => header_value(&m.raw, "subject").contains(&t.to_lowercase()),
        SearchKey::Seen => has(&Flag::Seen),
        SearchKey::Unseen => !has(&Flag::Seen),
        SearchKey::Flagged => has(&Flag::Flagged),
        SearchKey::Deleted => has(&Flag::Deleted),
        SearchKey::Larger(n) => m.raw.len() as u64 > *n,
        SearchKey::Smaller(n) => (m.raw.len() as u64) < *n,
        SearchKey::Uid(set) => set.contains(uid),
        SearchKey::UidFrom(n) => uid >= *n,
        SearchKey::Not(k) => !matches(k, uid, m),
        SearchKey::Or(a, b) => matches(a, uid, m) || matches(b, uid, m),
        SearchKey::And(keys) => keys.iter().all(|k| matches(k, uid, m)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Credentials, Secret, Security};

    const USER: &str = "alice@test.local";
    const PASS: &str = "correct horse";

    fn config() -> ConnectConfig {
        ConnectConfig {
            host: "fake.test".into(),
            port: 993,
            security: Security::Tls,
            pinned_cert_sha256: None,
            connect_timeout: Duration::from_secs(5),
        }
    }

    fn creds(password: &str) -> Credentials {
        Credentials::Password {
            username: USER.into(),
            password: Secret::new(password),
        }
    }

    fn server(caps: &[&str]) -> FakeServer {
        let s = FakeServer::new(caps);
        s.add_user(USER, PASS);
        s
    }

    fn msg(n: u32) -> FakeMessage {
        FakeMessage::simple(
            &format!("m{n}@test.local"),
            "Bob <bob@test.local>",
            USER,
            &format!("Subject {n}"),
            &format!("Body {n}"),
            1_792_411_200 + i64::from(n),
        )
    }

    #[tokio::test]
    async fn login_checks_password_and_revocation() {
        let s = server(&["IMAP4rev1"]);
        assert_eq!(
            s.connect(&config(), &creds("wrong")).await.err(),
            Some(ImapError::AuthFailed)
        );
        assert!(s.connect(&config(), &creds(PASS)).await.is_ok());
        s.set_password(USER, "rotated");
        assert_eq!(
            s.connect(&config(), &creds(PASS)).await.err(),
            Some(ImapError::AuthFailed)
        );
    }

    #[tokio::test]
    async fn select_fetch_and_search() {
        let s = server(&["IMAP4rev1"]);
        for n in 1..=3 {
            s.deliver("INBOX", msg(n)).unwrap();
        }
        s.set_flags("INBOX", 2, vec![Flag::Seen]).unwrap();
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();

        assert_eq!(
            t.uid_search(&SearchKey::All).await.err(),
            Some(ImapError::NotSelected)
        );
        let info = t.select("INBOX").await.unwrap();
        assert_eq!((info.uidvalidity, info.uidnext, info.exists), (1, 4, 3));
        assert_eq!(info.highest_modseq, None, "no CONDSTORE, no modseq");
        assert!(info.allows_new_keywords());

        assert_eq!(t.uid_search(&SearchKey::Unseen).await.unwrap(), vec![1, 3]);
        let subject = SearchKey::Subject("subject 3".into());
        assert_eq!(t.uid_search(&subject).await.unwrap(), vec![3]);
        let either = SearchKey::Or(
            Box::new(SearchKey::Uid(UidSet::single(1))),
            Box::new(subject),
        );
        assert_eq!(t.uid_search(&either).await.unwrap(), vec![1, 3]);
        let since = SearchKey::Since(ImapDate {
            year: 2026,
            month: 10,
            day: 19,
        });
        assert_eq!(t.uid_search(&since).await.unwrap(), vec![1, 2, 3]);
        let raw = SearchKey::GmailRaw("x".into());
        assert_eq!(
            t.uid_search(&raw).await.err(),
            Some(ImapError::Unsupported("X-GM-EXT-1"))
        );

        let headers = t
            .uid_fetch_headers(&UidSet::new([2, 3, 99]), HeaderFetch::default())
            .await
            .unwrap();
        assert_eq!(
            headers.iter().map(|h| h.uid).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(headers[0].flags, vec![Flag::Seen]);
        assert!(String::from_utf8_lossy(&headers[0].headers).contains("Subject: Subject 2"));
        assert!(!String::from_utf8_lossy(&headers[0].headers).contains("Body 2"));

        let text = t.uid_fetch_part(3, "1", None).await.unwrap();
        assert_eq!(text, b"Body 3\r\n");
        assert_eq!(t.uid_fetch_part(3, "1", Some(4)).await.unwrap(), b"Body");
    }

    #[tokio::test]
    async fn condstore_and_qresync_report_only_changes() {
        let s = server(&["IMAP4rev1", "QRESYNC"]);
        for n in 1..=3 {
            s.deliver("INBOX", msg(n)).unwrap();
        }
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        let modseq = t.select("INBOX").await.unwrap().highest_modseq.unwrap();

        s.set_flags("INBOX", 1, vec![Flag::Flagged]).unwrap();
        s.expunge_external("INBOX", 3).unwrap();

        let all = UidSet::new([1, 2, 3]);
        let sync = t.uid_fetch_flags(&all, Some(modseq)).await.unwrap();
        assert_eq!(sync.updates.len(), 1);
        assert_eq!(sync.updates[0].uid, 1);
        assert_eq!(sync.updates[0].flags, vec![Flag::Flagged]);
        assert_eq!(sync.vanished, vec![3]);
    }

    #[tokio::test]
    async fn move_requires_capability_and_returns_copyuid_with_uidplus() {
        let s = server(&["IMAP4rev1"]);
        s.add_folder("Archive", Some(SpecialUse::Archive));
        s.deliver("INBOX", msg(1)).unwrap();
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        assert_eq!(
            t.uid_move(&UidSet::single(1), "Archive").await.err(),
            Some(ImapError::Unsupported("MOVE"))
        );
        assert_eq!(
            t.uid_copy(&UidSet::single(1), "Archive").await.unwrap(),
            None,
            "no UIDPLUS, no COPYUID"
        );

        let s = server(&["IMAP4rev1", "MOVE", "UIDPLUS"]);
        s.add_folder("Archive", Some(SpecialUse::Archive));
        s.deliver("INBOX", msg(1).with_flags([Flag::Seen])).unwrap();
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        let copy = t
            .uid_move(&UidSet::single(1), "Archive")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((copy.source, copy.dest), (vec![1], vec![1]));
        assert!(s.uids("INBOX").is_empty());
        assert_eq!(
            s.flags("Archive", 1),
            Some(vec![Flag::Seen]),
            "flags survive a move"
        );
    }

    #[tokio::test]
    async fn uid_expunge_is_targeted_and_bare_expunge_is_not() {
        let s = server(&["IMAP4rev1", "UIDPLUS"]);
        for n in 1..=3 {
            s.deliver("INBOX", msg(n).with_flags([Flag::Deleted]))
                .unwrap();
        }
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        t.uid_expunge(&UidSet::single(2)).await.unwrap();
        assert_eq!(s.uids("INBOX"), vec![1, 3]);
        t.expunge().await.unwrap();
        assert!(s.uids("INBOX").is_empty());

        let s = server(&["IMAP4rev1"]);
        s.deliver("INBOX", msg(1)).unwrap();
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        assert_eq!(
            t.uid_expunge(&UidSet::single(1)).await.err(),
            Some(ImapError::Unsupported("UIDPLUS"))
        );
        assert!(s.calls().iter().any(|c| c.op == Op::UidExpunge));
        assert!(!s.calls().iter().any(|c| c.op == Op::Expunge));
    }

    #[tokio::test]
    async fn uidvalidity_change_renumbers() {
        let s = server(&["IMAP4rev1"]);
        for n in 1..=3 {
            s.deliver("INBOX", msg(n)).unwrap();
        }
        s.expunge_external("INBOX", 1).unwrap();
        s.change_uidvalidity("INBOX", 77).unwrap();
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        let info = t.select("INBOX").await.unwrap();
        assert_eq!((info.uidvalidity, info.uidnext), (77, 3));
        assert_eq!(s.uids("INBOX"), vec![1, 2]);
    }

    #[tokio::test]
    async fn append_and_folder_commands() {
        let s = server(&["IMAP4rev1", "UIDPLUS"]);
        s.add_folder("Sent", Some(SpecialUse::Sent));
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        let appended = t
            .append("Sent", &[Flag::Seen], b"Subject: x\r\n\r\nbody")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(appended.uid, 1);
        assert_eq!(s.flags("Sent", 1), Some(vec![Flag::Seen]));

        t.create("Café/日本語").await.unwrap();
        assert!(t.create("Café/日本語").await.is_err());
        t.rename("Café/日本語", "Café/中文").await.unwrap();
        assert!(s.folder_paths().contains(&"Café/中文".to_string()));
        t.delete("Café/中文").await.unwrap();
        assert!(!s.folder_paths().iter().any(|p| p.starts_with("Café/")));
        assert!(t.delete("INBOX").await.is_err());
    }

    #[tokio::test]
    async fn list_reports_roles_only_with_special_use() {
        let plain = server(&["IMAP4rev1"]);
        plain.add_folder("Trash", Some(SpecialUse::Trash));
        let mut t = plain.connect(&config(), &creds(PASS)).await.unwrap();
        let list = t.list().await.unwrap();
        let trash = list.iter().find(|m| m.path == "Trash").unwrap();
        assert_eq!(trash.special_use, None);
        assert!(trash.status.is_none());
        let inbox = list.iter().find(|m| m.path == "INBOX").unwrap();
        assert_eq!(inbox.special_use, Some(SpecialUse::Inbox));

        let rich = server(&["IMAP4rev1", "SPECIAL-USE", "LIST-STATUS"]);
        rich.add_folder("Trash", Some(SpecialUse::Trash));
        let mut t = rich.connect(&config(), &creds(PASS)).await.unwrap();
        let list = t.list().await.unwrap();
        let trash = list.iter().find(|m| m.path == "Trash").unwrap();
        assert_eq!(trash.special_use, Some(SpecialUse::Trash));
        assert_eq!(trash.attributes, vec!["\\Trash".to_string()]);
        assert!(trash.status.is_some() && trash.selectable());
    }

    #[tokio::test]
    async fn gmail_copies_share_msgid_and_labels() {
        let s = server(&["IMAP4rev1", "X-GM-EXT-1", "UIDPLUS"]);
        s.add_folder("[Gmail]/All Mail", Some(SpecialUse::All));
        s.add_folder("Work", None);
        s.deliver("INBOX", msg(1)).unwrap();
        s.deliver("[Gmail]/All Mail", msg(1).with_gmail_ids(1_001, 1_001))
            .unwrap();
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        let first = t
            .uid_copy(&UidSet::single(1), "Work")
            .await
            .unwrap()
            .unwrap();
        let again = t
            .uid_copy(&UidSet::single(1), "Work")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.dest, again.dest, "Gmail de-duplicates by msgid");
        assert_eq!(s.uids("Work").len(), 1);

        let h = t
            .uid_fetch_headers(&UidSet::single(1), HeaderFetch { gmail: true })
            .await
            .unwrap();
        assert_eq!(h[0].gm_msgid, Some(1_001));
        assert_eq!(
            h[0].gm_labels,
            vec!["\\Inbox".to_string(), "Work".to_string()]
        );
        let h = t
            .uid_fetch_headers(&UidSet::single(1), HeaderFetch::default())
            .await
            .unwrap();
        assert_eq!(h[0].gm_msgid, None, "Gmail attributes only when requested");
    }

    #[tokio::test]
    async fn scripted_failures_fire_once_in_order() {
        let s = server(&["IMAP4rev1"]);
        s.fail_next(Op::Connect, ImapError::Timeout);
        s.fail_next(Op::Select, ImapError::Bye);
        assert_eq!(
            s.connect(&config(), &creds(PASS)).await.err(),
            Some(ImapError::Timeout)
        );
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        assert_eq!(t.select("INBOX").await.err(), Some(ImapError::Bye));
        assert!(t.select("INBOX").await.is_ok());
        let ops: Vec<Op> = s.calls().iter().map(|c| c.op).collect();
        assert_eq!(ops, vec![Op::Connect, Op::Connect, Op::Select, Op::Select]);
    }

    #[tokio::test]
    async fn changes_since_cover_every_uid_up_to_the_limit() {
        let s = server(&["IMAP4rev1", "QRESYNC"]);
        for n in 1..=4 {
            s.deliver("INBOX", msg(n)).unwrap();
        }
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        let modseq = t.select("INBOX").await.unwrap().highest_modseq.unwrap();
        s.set_flags("INBOX", 1, vec![Flag::Seen]).unwrap();
        s.set_flags("INBOX", 4, vec![Flag::Seen]).unwrap();
        s.expunge_external("INBOX", 2).unwrap();
        let sync = t.uid_fetch_changes(3, modseq).await.unwrap();
        assert_eq!(
            sync.updates.iter().map(|u| u.uid).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(sync.vanished, vec![2]);

        let plain = server(&["IMAP4rev1"]);
        let mut t = plain.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        assert_eq!(
            t.uid_fetch_changes(10, 1).await.err(),
            Some(ImapError::Unsupported("CONDSTORE"))
        );
    }

    #[tokio::test]
    async fn fail_after_lets_calls_through_first() {
        let s = server(&["IMAP4rev1"]);
        s.fail_after(Op::Noop, 2, ImapError::Timeout);
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        assert!(t.noop().await.is_ok());
        assert!(t.noop().await.is_ok());
        assert_eq!(t.noop().await.err(), Some(ImapError::Timeout));
        assert!(t.noop().await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn idle_wakes_for_own_folder_and_times_out() {
        let s = server(&["IMAP4rev1", "IDLE"]);
        s.add_folder("Other", None);
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();

        let delivering = s.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            delivering.deliver("Other", msg(1)).unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
            delivering.deliver("INBOX", msg(2)).unwrap();
        });
        assert_eq!(
            t.idle(Duration::from_secs(60)).await.unwrap(),
            IdleEvent::Changed
        );
        handle.await.unwrap();
        assert_eq!(
            t.idle(Duration::from_secs(60)).await.unwrap(),
            IdleEvent::TimedOut
        );

        let no_idle = server(&["IMAP4rev1"]);
        let mut t = no_idle.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        assert_eq!(
            t.idle(Duration::from_secs(1)).await.err(),
            Some(ImapError::Unsupported("IDLE"))
        );
    }

    #[tokio::test]
    async fn mime_messages_are_described_and_fetched_by_part() {
        let raw = "Message-ID: <m@x>\r\nContent-Type: multipart/mixed; boundary=\"b0\"\r\n\r\n\
            preamble\r\n--b0\r\nContent-Type: multipart/alternative; boundary=b1\r\n\r\n\
            --b1\r\nContent-Type: text/plain; charset=utf-8; format=flowed\r\n\r\nplain \r\nbody\r\n\
            --b1\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\nPGI+aGk8L2I+\r\n\
            --b1--\r\n--b0\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Disposition: inline\r\n\
            Content-Transfer-Encoding: base64\r\n\r\niVBORw==\r\n--b0\r\nContent-Type: application/pdf; name=\"r.pdf\"\r\n\
            Content-Disposition: attachment; filename*=UTF-8''r%C3%A9.pdf\r\n\r\nPDF\r\n--b0--\r\n";
        let m = FakeMessage::mime(raw, 1);
        let ids: Vec<&str> = m.body.leaves().iter().map(|p| p.part_id.as_str()).collect();
        assert_eq!(ids, vec!["1.1", "1.2", "2", "3"]);
        let leaves = m.body.leaves();
        assert!(leaves[0].flowed);
        assert_eq!(leaves[1].encoding.as_deref(), Some("base64"));
        assert_eq!(leaves[2].content_id.as_deref(), Some("logo@x"));
        assert_eq!(leaves[3].filename.as_deref(), Some("ré.pdf"));
        assert_eq!(leaves[3].disposition.as_deref(), Some("attachment"));

        let s = server(&["IMAP4rev1"]);
        let uid = s.deliver("INBOX", m).unwrap();
        let mut t = s.connect(&config(), &creds(PASS)).await.unwrap();
        t.select("INBOX").await.unwrap();
        assert_eq!(
            t.uid_fetch_part(uid, "1.1", None).await.unwrap(),
            b"plain \r\nbody"
        );
        assert_eq!(
            t.uid_fetch_part(uid, "1.2", None).await.unwrap(),
            b"PGI+aGk8L2I+"
        );
        assert_eq!(t.uid_fetch_part(uid, "3", Some(2)).await.unwrap(), b"PD");
        assert!(t.uid_fetch_part(uid, "4", None).await.is_err());
        assert!(t
            .uid_fetch_part(uid, "", None)
            .await
            .unwrap()
            .starts_with(b"Message-ID"));
    }

    #[test]
    fn header_parsing_unfolds() {
        let raw = b"Subject: Hello\r\n  world\r\nFrom: a@b\r\n\r\nbody";
        assert_eq!(header_value(raw, "subject"), " hello  world");
        assert_eq!(header_value(raw, "FROM"), " a@b");
        assert_eq!(split_header(raw).1, b"body");
    }
}
