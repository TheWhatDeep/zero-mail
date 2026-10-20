//! The real IMAP transport over `async-imap` (section 2). This file and `parse.rs` are the
//! only places that import the crate, so it can be swapped without touching `sync`.
//!
//! Commands are sent with `run_command`, and their responses are read one by one and handed
//! to a per-command callback, so every response code (COPYUID, APPENDUID, HIGHESTMODSEQ) and
//! every untagged response (VANISHED) is visible. Each command runs under the section 12
//! timeout; after a timeout or a broken stream the connection is unusable and every later
//! call fails, so the caller reconnects.

use std::time::Duration;

use async_imap::imap_proto::{AttributeValue, MailboxDatum, Response, ResponseCode, Status};
use async_imap::{Authenticator, Client};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::client::TlsStream;

use crate::net::{tcp_connect, tls_handshake};
use crate::parse;
use crate::trace::Traced;
use crate::transport::{ConnectConfig, Credentials, ImapConnector, ImapTransport};
use crate::types::{
    AppendUid, BodyPart, Capabilities, CopyUid, FetchedHeader, Flag, FlagSync, FlagUpdate,
    HeaderFetch, IdleEvent, MailboxInfo, MailboxStatus, SearchKey, Security, SelectInfo,
    SpecialUse, StoreOp, UidSet,
};
use crate::{utf7, ErrorClass, ImapError, COMMAND_TIMEOUT, IDLE_READ_TIMEOUT, LARGE_FETCH_TIMEOUT};

type Stream = Traced<TlsStream<TcpStream>>;
type Session = async_imap::Session<Stream>;

/// Reads the next response from a client or session. The end of the stream or an untagged
/// BYE returns from the enclosing function, since the connection is over. (A macro, because
/// async-imap does not export its response type.)
macro_rules! read_next {
    ($conn:expr) => {{
        let r = $conn
            .read_response()
            .await
            .map_err(io_error)?
            .ok_or(ImapError::Bye)?;
        if let Response::Data {
            status: Status::Bye,
            outcome,
        } = r.parsed()
        {
            return Err(bye(outcome.information.as_deref()));
        }
        r
    }};
}

/// Header fields fetched with every new message (section 6, step 4).
pub const HEADER_FIELDS: &[&str] = &[
    "DATE",
    "FROM",
    "TO",
    "CC",
    "REPLY-TO",
    "SUBJECT",
    "MESSAGE-ID",
    "IN-REPLY-TO",
    "REFERENCES",
    "LIST-ID",
    "LIST-UNSUBSCRIBE",
    "LIST-UNSUBSCRIBE-POST",
];

/// Longest server text kept in an error.
const MAX_SERVER_TEXT: usize = 300;
/// Most UIDs one VANISHED reply may expand to.
const MAX_VANISHED: usize = 1_000_000;

/// Connects to real IMAP servers.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImapClient;

impl ImapConnector for ImapClient {
    type Transport = ImapSession;

    async fn probe(&self, config: &ConnectConfig) -> Result<Capabilities, ImapError> {
        let (mut client, caps) = open(config).await?;
        // Best effort: the probe already has what it came for.
        let _ = raw_command(&mut client, "Z9 LOGOUT", config.connect_timeout, |_| {}).await;
        Ok(caps)
    }

    async fn connect(
        &self,
        config: &ConnectConfig,
        credentials: &Credentials,
    ) -> Result<ImapSession, ImapError> {
        let (client, pre_auth) = open(config).await?;
        let session = tokio::time::timeout(COMMAND_TIMEOUT, login(client, &pre_auth, credentials))
            .await
            .map_err(|_| ImapError::Timeout)??;
        let mut transport = ImapSession {
            session: Some(session),
            caps: Capabilities::default(),
            broken: false,
        };
        let mut caps = None;
        transport
            .exec(Cmd::new("CAPABILITY"), COMMAND_TIMEOUT, |r| {
                if let Response::Capabilities(c) = r {
                    caps = Some(parse::capabilities(c));
                }
            })
            .await?;
        transport.caps = caps.unwrap_or(pre_auth);
        if transport.caps.qresync {
            transport
                .exec(Cmd::new("ENABLE QRESYNC"), COMMAND_TIMEOUT, |_| {})
                .await?;
        }
        Ok(transport)
    }
}

/// TCP, TLS (implicit or required STARTTLS), greeting and pre-login capabilities.
async fn open(config: &ConnectConfig) -> Result<(Client<Stream>, Capabilities), ImapError> {
    let timeout = config.connect_timeout;
    let tcp = tcp_connect(&config.host, config.port, timeout).await?;
    let tcp = match config.security {
        Security::Tls => tcp,
        Security::StartTls => {
            let mut plain = Client::new(tcp);
            let caps = match read_greeting(&mut plain, timeout).await? {
                Some(caps) => caps,
                None => raw_capability(&mut plain, "Z1", timeout).await?,
            };
            if !caps.starttls {
                return Err(ImapError::StartTlsUnavailable);
            }
            raw_command(&mut plain, "Z2 STARTTLS", timeout, |_| {}).await?;
            plain.into_inner()
        }
    };
    let tls = tls_handshake(tcp, &config.host, config.pinned_cert_sha256, timeout).await?;
    let mut client = Client::new(Traced::new(tls));
    let caps = match config.security {
        Security::Tls => match read_greeting(&mut client, timeout).await? {
            Some(caps) => caps,
            None => raw_capability(&mut client, "Z3", timeout).await?,
        },
        // Capabilities seen before TLS are discarded (RFC 3501, section 6.2.1).
        Security::StartTls => raw_capability(&mut client, "Z3", timeout).await?,
    };
    Ok((client, caps))
}

async fn read_greeting<T>(
    client: &mut Client<T>,
    timeout: Duration,
) -> Result<Option<Capabilities>, ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let read = async {
        let greeting = read_next!(client);
        match greeting.parsed() {
            Response::Data {
                status: Status::Ok,
                outcome,
            } => Ok(match &outcome.code {
                Some(ResponseCode::Capabilities(c)) => Some(parse::capabilities(c)),
                _ => None,
            }),
            _ => Err(ImapError::Protocol("unexpected greeting".into())),
        }
    };
    tokio::time::timeout(timeout, read)
        .await
        .map_err(|_| ImapError::Timeout)?
}

/// Sends one pre-login command with a fixed tag and waits for its tagged reply, passing
/// every untagged response to `on`.
async fn raw_command<T>(
    client: &mut Client<T>,
    line: &str,
    timeout: Duration,
    mut on: impl FnMut(&Response<'_>) + Send,
) -> Result<(), ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let tag = line.split(' ').next().unwrap_or_default().to_string();
    let run = async {
        let stream = client.get_mut();
        stream.write_all(line.as_bytes()).await.map_err(io_error)?;
        stream.write_all(b"\r\n").await.map_err(io_error)?;
        stream.flush().await.map_err(io_error)?;
        loop {
            let r = read_next!(client);
            match r.parsed() {
                Response::Done {
                    tag: t,
                    status,
                    outcome,
                } if t.0 == tag => return check_status(status, outcome.information.as_deref()),
                other => on(other),
            }
        }
    };
    tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| ImapError::Timeout)?
}

async fn raw_capability<T>(
    client: &mut Client<T>,
    tag: &str,
    timeout: Duration,
) -> Result<Capabilities, ImapError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let mut caps = None;
    raw_command(client, &format!("{tag} CAPABILITY"), timeout, |r| {
        if let Response::Capabilities(c) = r {
            caps = Some(parse::capabilities(c));
        }
    })
    .await?;
    caps.ok_or_else(|| ImapError::Protocol("no CAPABILITY response".into()))
}

/// `AUTHENTICATE PLAIN` when advertised, else `LOGIN` unless `LOGINDISABLED`; `XOAUTH2` for
/// OAuth (section 5). Exactly one attempt: a rejected login is never retried.
async fn login(
    client: Client<Stream>,
    caps: &Capabilities,
    credentials: &Credentials,
) -> Result<Session, ImapError> {
    let result = match credentials {
        Credentials::Password { username, password } => {
            if caps.auth_plain {
                let auth = Sasl::new(format!("\0{username}\0{}", password.expose()).into_bytes());
                client.authenticate("PLAIN", auth).await
            } else if !caps.login_disabled {
                client.login(username, password.expose()).await
            } else {
                return Err(ImapError::Unsupported("AUTH=PLAIN"));
            }
        }
        Credentials::XOAuth2 {
            username,
            access_token,
        } => {
            if !caps.auth_xoauth2 {
                return Err(ImapError::Unsupported("AUTH=XOAUTH2"));
            }
            let auth = Sasl::new(xoauth2_message(username, access_token.expose()));
            client.authenticate("XOAUTH2", auth).await
        }
    };
    result.map_err(|(err, _client)| login_error(err))
}

fn login_error(err: async_imap::error::Error) -> ImapError {
    use async_imap::error::Error;
    match err {
        Error::No(text) | Error::Bad(text) if parse::is_throttle(&text) => ImapError::Throttled,
        Error::No(_) | Error::Bad(_) | Error::Validate(_) => ImapError::AuthFailed,
        other => from_async(other),
    }
}

/// The SASL XOAUTH2 initial response.
pub(crate) fn xoauth2_message(username: &str, token: &str) -> Vec<u8> {
    format!("user={username}\x01auth=Bearer {token}\x01\x01").into_bytes()
}

/// Answers the first challenge with the prepared message and any later one (an XOAUTH2
/// error report) with an empty response, which ends the exchange.
struct Sasl {
    message: Vec<u8>,
    sent: bool,
}

impl Sasl {
    fn new(message: Vec<u8>) -> Self {
        Self {
            message,
            sent: false,
        }
    }
}

impl Authenticator for Sasl {
    type Response = Vec<u8>;

    fn process(&mut self, _challenge: &[u8]) -> Vec<u8> {
        if self.sent {
            Vec::new()
        } else {
            self.sent = true;
            std::mem::take(&mut self.message)
        }
    }
}

/// A command as text pieces and literals (`{n}` followed by raw bytes).
#[derive(Debug, Default)]
struct Cmd(Vec<Piece>);

#[derive(Debug)]
enum Piece {
    Text(String),
    Literal(Vec<u8>),
}

impl Cmd {
    fn new(text: impl Into<String>) -> Self {
        Self(vec![Piece::Text(text.into())])
    }

    fn text(mut self, text: impl Into<String>) -> Self {
        self.0.push(Piece::Text(text.into()));
        self
    }

    /// A mailbox name: modified UTF-7, then quoted.
    fn mailbox(self, name: &str) -> Self {
        let wire = utf7::encode(name);
        self.text(quote(&wire))
    }

    /// A string argument: quoted when it is plain ASCII, otherwise a literal.
    fn string(self, value: &str) -> Self {
        if is_quotable(value) {
            self.text(quote(value))
        } else {
            self.literal(value.as_bytes().to_vec())
        }
    }

    fn literal(mut self, bytes: Vec<u8>) -> Self {
        self.0.push(Piece::Literal(bytes));
        self
    }

    fn has_literal(&self) -> bool {
        self.0.iter().any(|p| matches!(p, Piece::Literal(_)))
    }

    /// Lines to send: each ends at a literal header, followed by that literal's bytes.
    fn lines(&self) -> Vec<(String, Option<&[u8]>)> {
        let mut lines = Vec::new();
        let mut current = String::new();
        for piece in &self.0 {
            match piece {
                Piece::Text(t) => current.push_str(t),
                Piece::Literal(bytes) => {
                    current.push_str(&format!("{{{}}}", bytes.len()));
                    lines.push((std::mem::take(&mut current), Some(bytes.as_slice())));
                }
            }
        }
        lines.push((current, None));
        lines
    }
}

fn is_quotable(value: &str) -> bool {
    value.len() < 1000 && value.bytes().all(|b| (0x20..0x7f).contains(&b))
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One authenticated connection.
#[derive(Debug)]
pub struct ImapSession {
    session: Option<Session>,
    caps: Capabilities,
    broken: bool,
}

impl ImapSession {
    fn session(&mut self) -> Result<&mut Session, ImapError> {
        if self.broken {
            return Err(ImapError::Network("connection is closed".into()));
        }
        self.session
            .as_mut()
            .ok_or_else(|| ImapError::Network("connection is closed".into()))
    }

    /// Runs one command. `on` sees every untagged response and, when the command succeeds,
    /// the tagged OK (whose response code may carry COPYUID or APPENDUID).
    async fn exec(
        &mut self,
        cmd: Cmd,
        timeout: Duration,
        mut on: impl FnMut(&Response<'_>) + Send,
    ) -> Result<(), ImapError> {
        let session = self.session()?;
        let result = match tokio::time::timeout(timeout, run(session, &cmd, &mut on)).await {
            Err(_) => Err(ImapError::Timeout),
            Ok(result) => result,
        };
        if let Err(err) = &result {
            if err.class() != ErrorClass::Permanent {
                self.broken = true;
            }
        }
        result
    }

    async fn do_idle(&mut self, max_wait: Duration) -> Result<IdleEvent, ImapError> {
        let session = self.session()?;
        let id = session.run_command("IDLE").await.map_err(from_async)?;
        let start = async {
            loop {
                let r = read_next!(session);
                match r.parsed() {
                    Response::Continue { .. } => return Ok(()),
                    Response::Done {
                        tag,
                        status,
                        outcome,
                    } if *tag == id => {
                        check_status(status, outcome.information.as_deref())?;
                        return Err(ImapError::Protocol("IDLE ended early".into()));
                    }
                    _ => {}
                }
            }
        };
        tokio::time::timeout(COMMAND_TIMEOUT, start)
            .await
            .map_err(|_| ImapError::Timeout)??;

        let deadline = Instant::now() + max_wait.min(IDLE_READ_TIMEOUT);
        let event = loop {
            let read = async {
                let r = read_next!(session);
                Ok::<bool, ImapError>(is_change(r.parsed()))
            };
            match tokio::time::timeout_at(deadline, read).await {
                Err(_) => break IdleEvent::TimedOut,
                Ok(changed) => {
                    if changed? {
                        break IdleEvent::Changed;
                    }
                }
            }
        };

        let done = async {
            let stream = session.get_mut();
            stream.write_all(b"DONE\r\n").await.map_err(io_error)?;
            stream.flush().await.map_err(io_error)?;
            loop {
                let r = read_next!(session);
                if let Response::Done {
                    tag,
                    status,
                    outcome,
                } = r.parsed()
                {
                    if *tag == id {
                        return check_status(status, outcome.information.as_deref());
                    }
                }
            }
        };
        tokio::time::timeout(COMMAND_TIMEOUT, done)
            .await
            .map_err(|_| ImapError::Timeout)??;
        Ok(event)
    }
}

/// Mailbox news that ends an IDLE wait (section 6, step 9).
fn is_change(r: &Response<'_>) -> bool {
    matches!(
        r,
        Response::Expunge(_)
            | Response::Vanished { .. }
            | Response::Fetch(..)
            | Response::MailboxData(MailboxDatum::Exists(_) | MailboxDatum::Recent(_))
    )
}

async fn run(
    session: &mut Session,
    cmd: &Cmd,
    on: &mut (impl FnMut(&Response<'_>) + Send),
) -> Result<(), ImapError> {
    let lines = cmd.lines();
    let (first, mut literal) = match lines.first() {
        Some((text, lit)) => (text.clone(), *lit),
        None => return Err(ImapError::Protocol("empty command".into())),
    };
    let id = session.run_command(&first).await.map_err(from_async)?;
    for (text, next_literal) in lines.iter().skip(1) {
        // Wait for the server's go-ahead before sending the literal.
        loop {
            let r = read_next!(session);
            match r.parsed() {
                Response::Continue { .. } => break,
                Response::Done {
                    tag,
                    status,
                    outcome,
                } if *tag == id => {
                    check_status(status, outcome.information.as_deref())?;
                    return Err(ImapError::Protocol("literal was not accepted".into()));
                }
                other => on(other),
            }
        }
        let stream = session.get_mut();
        stream
            .write_all(literal.unwrap_or_default())
            .await
            .map_err(io_error)?;
        stream.write_all(text.as_bytes()).await.map_err(io_error)?;
        stream.write_all(b"\r\n").await.map_err(io_error)?;
        stream.flush().await.map_err(io_error)?;
        literal = *next_literal;
    }
    loop {
        let r = read_next!(session);
        match r.parsed() {
            done @ Response::Done {
                tag,
                status,
                outcome,
            } if *tag == id => {
                check_status(status, outcome.information.as_deref())?;
                on(done);
                return Ok(());
            }
            other => on(other),
        }
    }
}

fn check_status(status: &Status, info: Option<&str>) -> Result<(), ImapError> {
    let text = server_text(info);
    match status {
        Status::Ok => Ok(()),
        Status::No if parse::is_throttle(&text) => Err(ImapError::Throttled),
        Status::No if is_missing_mailbox(&text) => Err(ImapError::NoSuchMailbox),
        Status::No => Err(ImapError::No(text)),
        Status::Bad => Err(ImapError::Bad(text)),
        Status::Bye => Err(bye(info)),
        Status::PreAuth => Err(ImapError::Protocol("unexpected PREAUTH".into())),
    }
}

fn is_missing_mailbox(text: &str) -> bool {
    let t = text.to_ascii_uppercase();
    t.contains("[NONEXISTENT]") || t.contains("[TRYCREATE]")
}

fn bye(info: Option<&str>) -> ImapError {
    if info.is_some_and(parse::is_throttle) {
        ImapError::Throttled
    } else {
        ImapError::Bye
    }
}

fn server_text(info: Option<&str>) -> String {
    info.unwrap_or_default()
        .chars()
        .take(MAX_SERVER_TEXT)
        .collect()
}

fn io_error(err: std::io::Error) -> ImapError {
    match err.kind() {
        std::io::ErrorKind::InvalidData => ImapError::Protocol("unparsable response".into()),
        std::io::ErrorKind::UnexpectedEof
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::BrokenPipe => ImapError::Bye,
        kind => ImapError::Network(kind.to_string()),
    }
}

fn from_async(err: async_imap::error::Error) -> ImapError {
    use async_imap::error::Error;
    match err {
        Error::Io(e) => io_error(e),
        Error::ConnectionLost => ImapError::Bye,
        Error::No(text) => match check_status(&Status::No, Some(&text)) {
            Err(e) => e,
            Ok(()) => ImapError::No(String::new()),
        },
        Error::Bad(text) => ImapError::Bad(server_text(Some(&text))),
        Error::Validate(_) => ImapError::Protocol("invalid character in a command".into()),
        _ => ImapError::Protocol("unparsable response".into()),
    }
}

/// A search key in IMAP syntax (sections 6 and 10).
fn search_cmd(cmd: Cmd, key: &SearchKey) -> Cmd {
    match key {
        SearchKey::All => cmd.text("ALL"),
        SearchKey::Since(d) => cmd.text(format!("SINCE {}", d.to_imap())),
        SearchKey::Before(d) => cmd.text(format!("BEFORE {}", d.to_imap())),
        SearchKey::Text(s) => cmd.text("TEXT ").string(s),
        SearchKey::From(s) => cmd.text("FROM ").string(s),
        SearchKey::To(s) => cmd.text("TO ").string(s),
        SearchKey::Cc(s) => cmd.text("CC ").string(s),
        SearchKey::Bcc(s) => cmd.text("BCC ").string(s),
        SearchKey::Subject(s) => cmd.text("SUBJECT ").string(s),
        SearchKey::Header(name, value) => cmd.text(format!("HEADER {name} ")).string(value),
        SearchKey::Seen => cmd.text("SEEN"),
        SearchKey::Unseen => cmd.text("UNSEEN"),
        SearchKey::Flagged => cmd.text("FLAGGED"),
        SearchKey::Deleted => cmd.text("DELETED"),
        SearchKey::Larger(n) => cmd.text(format!("LARGER {n}")),
        SearchKey::Smaller(n) => cmd.text(format!("SMALLER {n}")),
        // UID 0 never matches: an empty set searches nothing.
        SearchKey::Uid(set) if set.is_empty() => cmd.text("UID 0"),
        SearchKey::Uid(set) => cmd.text(format!("UID {}", set.to_imap())),
        SearchKey::UidFrom(n) => cmd.text(format!("UID {}:*", (*n).max(1))),
        SearchKey::Not(inner) => search_cmd(cmd.text("NOT "), inner),
        SearchKey::Or(a, b) => search_cmd(search_cmd(cmd.text("OR "), a).text(" "), b),
        SearchKey::And(keys) if keys.is_empty() => cmd.text("ALL"),
        SearchKey::And(keys) => {
            let mut cmd = cmd.text("(");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    cmd = cmd.text(" ");
                }
                cmd = search_cmd(cmd, k);
            }
            cmd.text(")")
        }
        SearchKey::GmailRaw(s) => cmd.text("X-GM-RAW ").string(s),
    }
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

fn flag_list(flags: &[Flag]) -> String {
    let names: Vec<&str> = flags.iter().map(Flag::as_imap).collect();
    format!("({})", names.join(" "))
}

/// A response code on the tagged reply or on an untagged OK.
fn response_code<'a>(r: &'a Response<'_>) -> Option<&'a ResponseCode<'a>> {
    match r {
        Response::Done { outcome, .. }
        | Response::Data {
            status: Status::Ok,
            outcome,
        } => outcome.code.as_ref(),
        _ => None,
    }
}

fn copy_uid(r: &Response<'_>) -> Option<CopyUid> {
    match response_code(r)? {
        ResponseCode::CopyUid(validity, src, dst) => Some(CopyUid {
            uidvalidity: *validity,
            source: parse::uid_members(src)?,
            dest: parse::uid_members(dst)?,
        }),
        _ => None,
    }
}

fn empty_header() -> FetchedHeader {
    FetchedHeader {
        uid: 0,
        flags: Vec::new(),
        internal_date: 0,
        size: 0,
        headers: Vec::new(),
        body: BodyPart::text_plain(0),
        modseq: None,
        gm_msgid: None,
        gm_thrid: None,
        gm_labels: Vec::new(),
    }
}

impl ImapTransport for ImapSession {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn list(&mut self) -> Result<Vec<MailboxInfo>, ImapError> {
        let mut cmd = Cmd::new(r#"LIST "" "*""#);
        if self.caps.list_status {
            cmd = cmd.text(if self.caps.special_use {
                " RETURN (SPECIAL-USE STATUS (MESSAGES UNSEEN UIDNEXT UIDVALIDITY))"
            } else {
                " RETURN (STATUS (MESSAGES UNSEEN UIDNEXT UIDVALIDITY))"
            });
        }
        let mut out: Vec<(String, MailboxInfo)> = Vec::new();
        let mut statuses: Vec<(String, MailboxStatus)> = Vec::new();
        self.exec(cmd, COMMAND_TIMEOUT, |r| match r {
            Response::MailboxData(MailboxDatum::List(data)) => {
                let mut attributes = Vec::new();
                let mut role = None;
                for a in &data.name_attributes {
                    let (text, r) = parse::name_attribute(a);
                    if !text.is_empty() {
                        attributes.push(text);
                    }
                    role = role.or(r);
                }
                let path = utf7::decode(&data.name);
                if path.eq_ignore_ascii_case("INBOX") {
                    role = Some(SpecialUse::Inbox);
                }
                out.push((
                    data.name.to_string(),
                    MailboxInfo {
                        path,
                        delimiter: data.delimiter.as_ref().and_then(|d| d.chars().next()),
                        attributes,
                        special_use: role,
                        status: None,
                    },
                ));
            }
            Response::MailboxData(MailboxDatum::Status { mailbox, status }) => {
                use async_imap::imap_proto::StatusAttribute as A;
                let mut s = MailboxStatus {
                    messages: 0,
                    unseen: 0,
                    uidnext: 0,
                    uidvalidity: 0,
                };
                for attr in status {
                    match attr {
                        A::Messages(n) => s.messages = *n,
                        A::Unseen(n) => s.unseen = *n,
                        A::UidNext(n) => s.uidnext = *n,
                        A::UidValidity(n) => s.uidvalidity = *n,
                        _ => {}
                    }
                }
                statuses.push((mailbox.to_string(), s));
            }
            _ => {}
        })
        .await?;
        for (raw, status) in statuses {
            if let Some((_, info)) = out.iter_mut().find(|(name, _)| *name == raw) {
                info.status = Some(status);
            }
        }
        Ok(out.into_iter().map(|(_, info)| info).collect())
    }

    async fn select(&mut self, path: &str) -> Result<SelectInfo, ImapError> {
        let mut cmd = Cmd::new("SELECT ").mailbox(path);
        if self.caps.condstore {
            cmd = cmd.text(" (CONDSTORE)");
        }
        let mut info = SelectInfo {
            uidvalidity: 0,
            uidnext: 0,
            exists: 0,
            highest_modseq: None,
            permanent_flags: Vec::new(),
        };
        self.exec(cmd, COMMAND_TIMEOUT, |r| match r {
            Response::MailboxData(MailboxDatum::Exists(n)) => info.exists = *n,
            Response::Data {
                status: Status::Ok,
                outcome,
            } => match &outcome.code {
                Some(ResponseCode::UidValidity(n)) => info.uidvalidity = *n,
                Some(ResponseCode::UidNext(n)) => info.uidnext = *n,
                Some(ResponseCode::HighestModSeq(n)) => info.highest_modseq = Some(*n),
                Some(ResponseCode::PermanentFlags(flags)) => {
                    info.permanent_flags = flags.iter().map(|f| f.to_string()).collect();
                }
                _ => {}
            },
            _ => {}
        })
        .await?;
        if !self.caps.condstore {
            info.highest_modseq = None;
        }
        if info.uidvalidity == 0 {
            return Err(ImapError::Protocol("SELECT returned no UIDVALIDITY".into()));
        }
        Ok(info)
    }

    async fn uid_search(&mut self, key: &SearchKey) -> Result<Vec<u32>, ImapError> {
        if contains_gmail_raw(key) && !self.caps.gmail {
            return Err(ImapError::Unsupported("X-GM-EXT-1"));
        }
        let mut cmd = Cmd::new("UID SEARCH ");
        if search_cmd(Cmd::default(), key).has_literal() {
            cmd = cmd.text("CHARSET UTF-8 ");
        }
        let mut uids = Vec::new();
        self.exec(search_cmd(cmd, key), COMMAND_TIMEOUT, |r| {
            if let Response::MailboxData(MailboxDatum::Search(found)) = r {
                uids.extend_from_slice(found);
            }
        })
        .await?;
        uids.sort_unstable();
        uids.dedup();
        Ok(uids)
    }

    async fn uid_fetch_headers(
        &mut self,
        uids: &UidSet,
        options: HeaderFetch,
    ) -> Result<Vec<FetchedHeader>, ImapError> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let gmail = options.gmail && self.caps.gmail;
        let mut items = format!(
            "UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS ({})]",
            HEADER_FIELDS.join(" ")
        );
        if self.caps.condstore {
            items.push_str(" MODSEQ");
        }
        if gmail {
            items.push_str(" X-GM-MSGID X-GM-THRID X-GM-LABELS");
        }
        let cmd = Cmd::new(format!("UID FETCH {} ({items})", uids.to_imap()));
        let mut out = Vec::new();
        self.exec(cmd, LARGE_FETCH_TIMEOUT, |r| {
            let Response::Fetch(_, attrs) = r else {
                return;
            };
            let mut h = empty_header();
            let mut has_structure = false;
            for a in attrs {
                match a {
                    AttributeValue::Uid(u) => h.uid = *u,
                    AttributeValue::Flags(f) => h.flags = parse::flags(f),
                    AttributeValue::InternalDate(d) => {
                        h.internal_date = parse::internal_date(d).unwrap_or(0);
                    }
                    AttributeValue::Rfc822Size(n) => h.size = *n,
                    AttributeValue::BodyStructure(bs) => {
                        h.body = parse::body_part(bs);
                        has_structure = true;
                    }
                    AttributeValue::BodySection { data, .. } => {
                        h.headers = data.as_deref().unwrap_or_default().to_vec();
                    }
                    AttributeValue::ModSeq(m) => h.modseq = Some(*m),
                    AttributeValue::GmailMsgId(id) => h.gm_msgid = Some(*id),
                    AttributeValue::GmailThrId(id) => h.gm_thrid = Some(*id),
                    AttributeValue::GmailLabels(labels) => {
                        h.gm_labels = labels.iter().map(|l| utf7::decode(l)).collect();
                    }
                    _ => {}
                }
            }
            // Unsolicited FETCH responses (flag changes) carry no structure; skip them.
            if h.uid != 0 && has_structure && uids.contains(h.uid) {
                out.push(h);
            }
        })
        .await?;
        Ok(out)
    }

    async fn uid_fetch_flags(
        &mut self,
        uids: &UidSet,
        changed_since: Option<u64>,
    ) -> Result<FlagSync, ImapError> {
        if uids.is_empty() {
            return Ok(FlagSync::default());
        }
        let condstore = self.caps.condstore;
        let mut text = format!(
            "UID FETCH {} (UID FLAGS{})",
            uids.to_imap(),
            if condstore { " MODSEQ" } else { "" }
        );
        if let (true, Some(modseq)) = (condstore, changed_since) {
            text.push_str(&format!(" (CHANGEDSINCE {modseq}"));
            if self.caps.qresync {
                text.push_str(" VANISHED");
            }
            text.push(')');
        }
        let mut sync = FlagSync::default();
        self.exec(Cmd::new(text), LARGE_FETCH_TIMEOUT, |r| match r {
            Response::Fetch(_, attrs) => {
                let mut update = FlagUpdate {
                    uid: 0,
                    flags: Vec::new(),
                    modseq: None,
                };
                let mut has_flags = false;
                for a in attrs {
                    match a {
                        AttributeValue::Uid(u) => update.uid = *u,
                        AttributeValue::Flags(f) => {
                            update.flags = parse::flags(f);
                            has_flags = true;
                        }
                        AttributeValue::ModSeq(m) => update.modseq = Some(*m),
                        _ => {}
                    }
                }
                if has_flags && uids.contains(update.uid) {
                    sync.updates.push(update);
                }
            }
            Response::Vanished { uids: ranges, .. } => {
                sync.vanished.extend(
                    uids.iter()
                        .filter(|uid| ranges.iter().any(|range| range.contains(uid))),
                );
            }
            _ => {}
        })
        .await?;
        sync.vanished.sort_unstable();
        sync.vanished.dedup();
        Ok(sync)
    }

    async fn uid_fetch_changes(
        &mut self,
        up_to: u32,
        changed_since: u64,
    ) -> Result<FlagSync, ImapError> {
        if !self.caps.condstore {
            return Err(ImapError::Unsupported("CONDSTORE"));
        }
        let up_to = up_to.max(1);
        let vanished = if self.caps.qresync { " VANISHED" } else { "" };
        let text = format!(
            "UID FETCH 1:{up_to} (UID FLAGS MODSEQ) (CHANGEDSINCE {changed_since}{vanished})"
        );
        let mut sync = FlagSync::default();
        let mut too_many = false;
        self.exec(Cmd::new(text), LARGE_FETCH_TIMEOUT, |r| match r {
            Response::Fetch(_, attrs) => {
                let mut update = FlagUpdate {
                    uid: 0,
                    flags: Vec::new(),
                    modseq: None,
                };
                let mut has_flags = false;
                for a in attrs {
                    match a {
                        AttributeValue::Uid(u) => update.uid = *u,
                        AttributeValue::Flags(f) => {
                            update.flags = parse::flags(f);
                            has_flags = true;
                        }
                        AttributeValue::ModSeq(m) => update.modseq = Some(*m),
                        _ => {}
                    }
                }
                if has_flags && update.uid != 0 && update.uid <= up_to {
                    sync.updates.push(update);
                }
            }
            Response::Vanished { uids: ranges, .. } => {
                for range in ranges {
                    let (lo, hi) = (*range.start(), (*range.end()).min(up_to));
                    if lo > hi {
                        continue;
                    }
                    if sync.vanished.len() + (hi - lo) as usize > MAX_VANISHED {
                        too_many = true;
                        continue;
                    }
                    sync.vanished.extend(lo..=hi);
                }
            }
            _ => {}
        })
        .await?;
        if too_many {
            return Err(ImapError::Protocol("VANISHED list too long".into()));
        }
        sync.vanished.sort_unstable();
        sync.vanished.dedup();
        Ok(sync)
    }

    async fn uid_fetch_part(
        &mut self,
        uid: u32,
        part: &str,
        max_bytes: Option<u64>,
    ) -> Result<Vec<u8>, ImapError> {
        let range = max_bytes.map(|n| format!("<0.{n}>")).unwrap_or_default();
        let cmd = Cmd::new(format!("UID FETCH {uid} (UID BODY.PEEK[{part}]{range})"));
        let mut found = None;
        self.exec(cmd, LARGE_FETCH_TIMEOUT, |r| {
            let Response::Fetch(_, attrs) = r else {
                return;
            };
            let ours = attrs
                .iter()
                .any(|a| matches!(a, AttributeValue::Uid(u) if *u == uid));
            let data = attrs.iter().find_map(|a| match a {
                AttributeValue::BodySection { data, .. } => Some(data),
                _ => None,
            });
            if let (true, Some(data)) = (ours, data) {
                found = Some(data.as_deref().unwrap_or_default().to_vec());
            }
        })
        .await?;
        found.ok_or_else(|| ImapError::No("no such message or part".into()))
    }

    async fn uid_store(
        &mut self,
        uids: &UidSet,
        op: StoreOp,
        flags: &[Flag],
    ) -> Result<(), ImapError> {
        if uids.is_empty() || flags.is_empty() {
            return Ok(());
        }
        let sign = match op {
            StoreOp::Add => '+',
            StoreOp::Remove => '-',
        };
        let cmd = Cmd::new(format!(
            "UID STORE {} {sign}FLAGS.SILENT {}",
            uids.to_imap(),
            flag_list(flags)
        ));
        self.exec(cmd, COMMAND_TIMEOUT, |_| {}).await
    }

    async fn uid_move(&mut self, uids: &UidSet, dest: &str) -> Result<Option<CopyUid>, ImapError> {
        if !self.caps.move_ {
            return Err(ImapError::Unsupported("MOVE"));
        }
        let cmd = Cmd::new(format!("UID MOVE {} ", uids.to_imap())).mailbox(dest);
        let mut copied = None;
        self.exec(cmd, COMMAND_TIMEOUT, |r| {
            copied = copied.take().or_else(|| copy_uid(r));
        })
        .await?;
        Ok(copied)
    }

    async fn uid_copy(&mut self, uids: &UidSet, dest: &str) -> Result<Option<CopyUid>, ImapError> {
        let cmd = Cmd::new(format!("UID COPY {} ", uids.to_imap())).mailbox(dest);
        let mut copied = None;
        self.exec(cmd, COMMAND_TIMEOUT, |r| {
            copied = copied.take().or_else(|| copy_uid(r));
        })
        .await?;
        Ok(copied)
    }

    async fn uid_expunge(&mut self, uids: &UidSet) -> Result<(), ImapError> {
        if !self.caps.uidplus {
            return Err(ImapError::Unsupported("UIDPLUS"));
        }
        if uids.is_empty() {
            return Ok(());
        }
        let cmd = Cmd::new(format!("UID EXPUNGE {}", uids.to_imap()));
        self.exec(cmd, COMMAND_TIMEOUT, |_| {}).await
    }

    async fn expunge(&mut self) -> Result<(), ImapError> {
        self.exec(Cmd::new("EXPUNGE"), COMMAND_TIMEOUT, |_| {})
            .await
    }

    async fn append(
        &mut self,
        folder: &str,
        flags: &[Flag],
        raw: &[u8],
    ) -> Result<Option<AppendUid>, ImapError> {
        let cmd = Cmd::new("APPEND ")
            .mailbox(folder)
            .text(format!(" {} ", flag_list(flags)))
            .literal(raw.to_vec());
        let mut appended = None;
        self.exec(cmd, LARGE_FETCH_TIMEOUT, |r| {
            if let Some(ResponseCode::AppendUid(validity, uids)) = response_code(r) {
                if let Some(uid) = parse::uid_members(uids).and_then(|u| u.first().copied()) {
                    appended = Some(AppendUid {
                        uidvalidity: *validity,
                        uid,
                    });
                }
            }
        })
        .await?;
        Ok(appended)
    }

    async fn create(&mut self, path: &str) -> Result<(), ImapError> {
        self.exec(Cmd::new("CREATE ").mailbox(path), COMMAND_TIMEOUT, |_| {})
            .await
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<(), ImapError> {
        let cmd = Cmd::new("RENAME ").mailbox(from).text(" ").mailbox(to);
        self.exec(cmd, COMMAND_TIMEOUT, |_| {}).await
    }

    async fn delete(&mut self, path: &str) -> Result<(), ImapError> {
        self.exec(Cmd::new("DELETE ").mailbox(path), COMMAND_TIMEOUT, |_| {})
            .await
    }

    async fn idle(&mut self, max_wait: Duration) -> Result<IdleEvent, ImapError> {
        if !self.caps.idle {
            return Err(ImapError::Unsupported("IDLE"));
        }
        let result = self.do_idle(max_wait).await;
        if result.is_err() {
            self.broken = true;
        }
        result
    }

    async fn noop(&mut self) -> Result<(), ImapError> {
        self.exec(Cmd::new("NOOP"), COMMAND_TIMEOUT, |_| {}).await
    }

    async fn logout(mut self) -> Result<(), ImapError> {
        match self.exec(Cmd::new("LOGOUT"), COMMAND_TIMEOUT, |_| {}).await {
            Ok(()) | Err(ImapError::Bye) => Ok(()),
            Err(other) => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImapDate;

    fn render(cmd: &Cmd) -> Vec<(String, Option<Vec<u8>>)> {
        cmd.lines()
            .into_iter()
            .map(|(t, l)| (t, l.map(<[u8]>::to_vec)))
            .collect()
    }

    #[test]
    fn mailbox_names_are_utf7_and_quoted() {
        let cmd = Cmd::new("SELECT ").mailbox("Café/\"x\"");
        assert_eq!(
            render(&cmd),
            vec![(r#"SELECT "Caf&AOk-/\"x\"""#.to_string(), None)]
        );
    }

    #[test]
    fn non_ascii_strings_become_literals() {
        let key = SearchKey::And(vec![
            SearchKey::Since(ImapDate {
                year: 2026,
                month: 7,
                day: 21,
            }),
            SearchKey::Subject("Grüße".into()),
            SearchKey::Not(Box::new(SearchKey::Deleted)),
        ]);
        assert!(search_cmd(Cmd::default(), &key).has_literal());
        let cmd = search_cmd(Cmd::new("UID SEARCH CHARSET UTF-8 "), &key);
        assert_eq!(
            render(&cmd),
            vec![
                (
                    "UID SEARCH CHARSET UTF-8 (SINCE 21-Jul-2026 SUBJECT {7}".to_string(),
                    Some("Grüße".as_bytes().to_vec())
                ),
                (" NOT DELETED)".to_string(), None),
            ]
        );
        let plain = search_cmd(Cmd::default(), &SearchKey::From("a\"b".into()));
        assert_eq!(render(&plain), vec![(r#"FROM "a\"b""#.to_string(), None)]);
        let or = search_cmd(
            Cmd::default(),
            &SearchKey::Or(
                Box::new(SearchKey::Uid(UidSet::new([1, 2, 3]))),
                Box::new(SearchKey::Unseen),
            ),
        );
        assert_eq!(render(&or), vec![("OR UID 1:3 UNSEEN".to_string(), None)]);
    }

    #[test]
    fn append_sends_the_message_as_a_literal() {
        let cmd = Cmd::new("APPEND ")
            .mailbox("Sent")
            .text(format!(" {} ", flag_list(&[Flag::Seen])))
            .literal(b"Subject: x\r\n\r\nhi".to_vec());
        assert_eq!(
            render(&cmd),
            vec![
                (
                    r#"APPEND "Sent" (\Seen) {16}"#.to_string(),
                    Some(b"Subject: x\r\n\r\nhi".to_vec())
                ),
                (String::new(), None),
            ]
        );
    }

    #[test]
    fn sasl_responses() {
        assert_eq!(
            xoauth2_message("me@example.com", "tok"),
            b"user=me@example.com\x01auth=Bearer tok\x01\x01".to_vec()
        );
        let mut sasl = Sasl::new(b"\0u\0p".to_vec());
        assert_eq!(sasl.process(b""), b"\0u\0p".to_vec());
        assert!(
            sasl.process(b"error").is_empty(),
            "a second challenge ends the exchange"
        );
    }

    #[test]
    fn status_mapping() {
        assert_eq!(
            check_status(&Status::No, Some("[LIMIT] slow down")),
            Err(ImapError::Throttled)
        );
        assert_eq!(
            check_status(&Status::No, Some("[NONEXISTENT] no such mailbox")),
            Err(ImapError::NoSuchMailbox)
        );
        assert_eq!(
            check_status(&Status::Bad, Some("syntax")),
            Err(ImapError::Bad("syntax".into()))
        );
        assert_eq!(check_status(&Status::Ok, None), Ok(()));
        assert_eq!(bye(Some("[UNAVAILABLE] maintenance")), ImapError::Throttled);
        assert_eq!(bye(Some("Logging out")), ImapError::Bye);
        assert_eq!(
            login_error(async_imap::error::Error::No(
                "[AUTHENTICATIONFAILED] Invalid".into()
            )),
            ImapError::AuthFailed
        );
    }

    /// A plaintext server that sends `greeting`, then answers each client line from
    /// `replies` in order. Returns the port and the lines the client sent.
    async fn plain_server(
        greeting: &'static str,
        replies: Vec<&'static str>,
    ) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut sock = BufReader::new(sock);
            sock.get_mut().write_all(greeting.as_bytes()).await.unwrap();
            let mut got = Vec::new();
            for reply in replies {
                let mut line = String::new();
                sock.read_line(&mut line).await.unwrap();
                got.push(line.trim_end().to_string());
                sock.get_mut().write_all(reply.as_bytes()).await.unwrap();
            }
            let mut rest = Vec::new();
            let _ = sock.read_to_end(&mut rest).await;
            got
        });
        (port, handle)
    }

    fn plain_config(port: u16, security: Security) -> ConnectConfig {
        ConnectConfig {
            host: "127.0.0.1".into(),
            port,
            security,
            pinned_cert_sha256: None,
            connect_timeout: Duration::from_secs(5),
        }
    }

    fn creds() -> Credentials {
        Credentials::Password {
            username: "u".into(),
            password: crate::Secret::new("p"),
        }
    }

    #[tokio::test]
    async fn starttls_not_offered_aborts_before_login() {
        let (port, handle) =
            plain_server("* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] hello\r\n", vec![]).await;
        let err = ImapClient
            .connect(&plain_config(port, Security::StartTls), &creds())
            .await
            .unwrap_err();
        assert_eq!(err, ImapError::StartTlsUnavailable);
        assert!(
            handle.await.unwrap().is_empty(),
            "nothing sent in plaintext"
        );
    }

    #[tokio::test]
    async fn starttls_capability_is_asked_for_when_the_greeting_has_none() {
        let (port, handle) = plain_server(
            "* OK hello\r\n",
            vec!["* CAPABILITY IMAP4rev1 LOGINDISABLED\r\nZ1 OK done\r\n"],
        )
        .await;
        let err = ImapClient
            .connect(&plain_config(port, Security::StartTls), &creds())
            .await
            .unwrap_err();
        assert_eq!(err, ImapError::StartTlsUnavailable);
        assert_eq!(handle.await.unwrap(), vec!["Z1 CAPABILITY".to_string()]);
    }

    #[tokio::test]
    async fn plaintext_server_on_an_implicit_tls_port_fails_tls() {
        let (port, _handle) = plain_server("* OK hello\r\n", vec![]).await;
        let err = ImapClient
            .connect(&plain_config(port, Security::Tls), &creds())
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ImapError::Tls(_) | ImapError::Network(_) | ImapError::Bye
            ),
            "{err:?}"
        );
        assert_ne!(err.class(), ErrorClass::Permanent);
    }
}
