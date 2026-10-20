//! Protocol-level values shared by every transport. Nothing here knows about the database.

use crate::MAX_UIDS_PER_COMMAND;

/// Transport security for one endpoint. There is deliberately no plaintext option (section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Implicit TLS (IMAP 993, SMTP 465).
    Tls,
    /// Required STARTTLS (IMAP 143, SMTP 587). Aborts if the server does not offer it.
    StartTls,
}

/// The capabilities the client uses, read from `CAPABILITY` (section 6). Everything is
/// optional: plain IMAP4rev1 with no extensions must work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub idle: bool,
    pub move_: bool,
    pub uidplus: bool,
    pub condstore: bool,
    pub qresync: bool,
    pub special_use: bool,
    pub list_status: bool,
    /// `APPENDLIMIT`. `Some(None)` means advertised without a global value.
    pub append_limit: Option<Option<u64>>,
    /// `X-GM-EXT-1` (Gmail quirks, section 9).
    pub gmail: bool,
    pub starttls: bool,
    pub login_disabled: bool,
    pub auth_plain: bool,
    pub auth_xoauth2: bool,
}

impl Capabilities {
    /// Parses capability atoms as sent by the server, case-insensitively. Unknown atoms are
    /// ignored.
    pub fn parse<'a>(atoms: impl IntoIterator<Item = &'a str>) -> Self {
        let mut caps = Self::default();
        for atom in atoms {
            let upper = atom.trim().to_ascii_uppercase();
            match upper.as_str() {
                "IDLE" => caps.idle = true,
                "MOVE" => caps.move_ = true,
                "UIDPLUS" => caps.uidplus = true,
                "CONDSTORE" => caps.condstore = true,
                // RFC 7162: QRESYNC implies CONDSTORE.
                "QRESYNC" => {
                    caps.qresync = true;
                    caps.condstore = true;
                }
                "SPECIAL-USE" => caps.special_use = true,
                "LIST-STATUS" => caps.list_status = true,
                "X-GM-EXT-1" => caps.gmail = true,
                "STARTTLS" => caps.starttls = true,
                "LOGINDISABLED" => caps.login_disabled = true,
                "AUTH=PLAIN" => caps.auth_plain = true,
                "AUTH=XOAUTH2" => caps.auth_xoauth2 = true,
                "APPENDLIMIT" => caps.append_limit = Some(None),
                other => {
                    if let Some(limit) = other.strip_prefix("APPENDLIMIT=") {
                        caps.append_limit = Some(limit.parse().ok());
                    }
                }
            }
        }
        caps
    }
}

/// RFC 6154 special-use roles, plus INBOX (section 4, `folders.special_use`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialUse {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    All,
}

impl SpecialUse {
    /// Maps a `LIST` attribute such as `\Sent` to a role.
    pub fn from_attribute(attr: &str) -> Option<Self> {
        match attr.to_ascii_lowercase().as_str() {
            "\\sent" => Some(Self::Sent),
            "\\drafts" => Some(Self::Drafts),
            "\\trash" => Some(Self::Trash),
            "\\junk" => Some(Self::Junk),
            "\\archive" => Some(Self::Archive),
            "\\all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_attribute(self) -> &'static str {
        match self {
            Self::Inbox => "",
            Self::Sent => "\\Sent",
            Self::Drafts => "\\Drafts",
            Self::Trash => "\\Trash",
            Self::Junk => "\\Junk",
            Self::Archive => "\\Archive",
            Self::All => "\\All",
        }
    }
}

/// `STATUS` counters, returned with `LIST` when the server supports `LIST-STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxStatus {
    pub messages: u32,
    pub unseen: u32,
    pub uidnext: u32,
    pub uidvalidity: u32,
}

/// One `LIST` entry. `path` is already decoded from modified UTF-7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxInfo {
    pub path: String,
    pub delimiter: Option<char>,
    pub attributes: Vec<String>,
    pub special_use: Option<SpecialUse>,
    pub status: Option<MailboxStatus>,
}

impl MailboxInfo {
    /// `\Noselect` or `\NonExistent` folders cannot hold messages.
    pub fn selectable(&self) -> bool {
        !self.attributes.iter().any(|a| {
            a.eq_ignore_ascii_case("\\Noselect") || a.eq_ignore_ascii_case("\\NonExistent")
        })
    }
}

/// The result of `SELECT` (section 6, step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectInfo {
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub exists: u32,
    /// Present only when the server supports CONDSTORE.
    pub highest_modseq: Option<u64>,
    pub permanent_flags: Vec<String>,
}

impl SelectInfo {
    /// `\*` in PERMANENTFLAGS means new keywords such as `$Muted` can be stored (18.5).
    pub fn allows_new_keywords(&self) -> bool {
        self.permanent_flags.iter().any(|f| f == "\\*")
    }
}

/// A message flag. The client maps these onto `messages.flags` bits (section 4).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Flag {
    Seen,
    Flagged,
    Answered,
    Draft,
    Deleted,
    /// Any keyword, for example `$Forwarded` or `$Muted`.
    Keyword(String),
}

impl Flag {
    pub fn parse(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "\\seen" => Self::Seen,
            "\\flagged" => Self::Flagged,
            "\\answered" => Self::Answered,
            "\\draft" => Self::Draft,
            "\\deleted" => Self::Deleted,
            _ => Self::Keyword(raw.to_string()),
        }
    }

    pub fn as_imap(&self) -> &str {
        match self {
            Self::Seen => "\\Seen",
            Self::Flagged => "\\Flagged",
            Self::Answered => "\\Answered",
            Self::Draft => "\\Draft",
            Self::Deleted => "\\Deleted",
            Self::Keyword(k) => k,
        }
    }
}

/// `UID STORE` mode. Always sent with `.SILENT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    Add,
    Remove,
}

/// A sorted, de-duplicated set of UIDs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UidSet(Vec<u32>);

impl UidSet {
    pub fn new(uids: impl IntoIterator<Item = u32>) -> Self {
        let mut v: Vec<u32> = uids.into_iter().filter(|u| *u != 0).collect();
        v.sort_unstable();
        v.dedup();
        Self(v)
    }

    pub fn single(uid: u32) -> Self {
        Self::new([uid])
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn contains(&self, uid: u32) -> bool {
        self.0.binary_search(&uid).is_ok()
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().copied()
    }

    /// Splits into sets of at most [`MAX_UIDS_PER_COMMAND`] UIDs (section 9).
    pub fn chunks(&self) -> Vec<UidSet> {
        self.0
            .chunks(MAX_UIDS_PER_COMMAND)
            .map(|c| UidSet(c.to_vec()))
            .collect()
    }

    /// IMAP sequence-set syntax with ranges compressed, for example `1:3,7,9:10`.
    pub fn to_imap(&self) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < self.0.len() {
            let start = self.0[i];
            let mut end = start;
            while i + 1 < self.0.len() && self.0[i + 1] == end + 1 {
                i += 1;
                end = self.0[i];
            }
            if !out.is_empty() {
                out.push(',');
            }
            if start == end {
                out.push_str(&start.to_string());
            } else {
                out.push_str(&format!("{start}:{end}"));
            }
            i += 1;
        }
        out
    }
}

/// One node of a parsed `BODYSTRUCTURE`. Multipart nodes have children and no size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyPart {
    /// IMAP part specifier, for example `1.2`. Empty for the root of a multipart message.
    pub part_id: String,
    /// Lowercase `type/subtype`.
    pub mime: String,
    pub charset: Option<String>,
    pub encoding: Option<String>,
    pub content_id: Option<String>,
    /// `inline` or `attachment` when a Content-Disposition is present.
    pub disposition: Option<String>,
    /// Filename from the disposition, else the `name` content-type parameter. An RFC 2231
    /// `filename*` value is decoded; RFC 2047 encoded words are left for the display layer.
    pub filename: Option<String>,
    pub size: u64,
    /// `format=flowed` (RFC 3676) on a text part; section 8 honours it.
    pub flowed: bool,
    /// `delsp=yes` alongside `format=flowed`.
    pub delsp: bool,
    pub children: Vec<BodyPart>,
}

impl BodyPart {
    /// A single `text/plain; charset=utf-8` part.
    pub fn text_plain(size: u64) -> Self {
        Self {
            part_id: "1".into(),
            mime: "text/plain".into(),
            charset: Some("utf-8".into()),
            encoding: Some("7bit".into()),
            content_id: None,
            disposition: None,
            filename: None,
            size,
            flowed: false,
            delsp: false,
            children: Vec::new(),
        }
    }

    /// Depth-first iteration over leaf parts.
    pub fn leaves(&self) -> Vec<&BodyPart> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves<'a>(&'a self, out: &mut Vec<&'a BodyPart>) {
        if self.children.is_empty() {
            out.push(self);
        } else {
            for c in &self.children {
                c.collect_leaves(out);
            }
        }
    }
}

/// Options for a header fetch (section 6, step 4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeaderFetch {
    /// Also request `X-GM-MSGID`, `X-GM-THRID`, `X-GM-LABELS` (section 9).
    pub gmail: bool,
}

/// One message from a header fetch: `UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE`
/// plus the selected header fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedHeader {
    pub uid: u32,
    pub flags: Vec<Flag>,
    /// INTERNALDATE as UTC epoch seconds.
    pub internal_date: i64,
    pub size: u32,
    /// Raw selected header fields, CRLF separated.
    pub headers: Vec<u8>,
    pub body: BodyPart,
    pub modseq: Option<u64>,
    pub gm_msgid: Option<u64>,
    pub gm_thrid: Option<u64>,
    pub gm_labels: Vec<String>,
}

/// One message's flags from a flag fetch (section 6, step 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagUpdate {
    pub uid: u32,
    pub flags: Vec<Flag>,
    pub modseq: Option<u64>,
}

/// Result of a flag fetch: changed flags and, with QRESYNC, `VANISHED` UIDs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlagSync {
    pub updates: Vec<FlagUpdate>,
    pub vanished: Vec<u32>,
}

/// `COPYUID` response code (UIDPLUS). `source[i]` was copied to `dest[i]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyUid {
    pub uidvalidity: u32,
    pub source: Vec<u32>,
    pub dest: Vec<u32>,
}

/// `APPENDUID` response code (UIDPLUS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendUid {
    pub uidvalidity: u32,
    pub uid: u32,
}

/// How an IDLE wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    /// `EXISTS`, `EXPUNGE` or `FETCH` arrived; rerun the folder procedure.
    Changed,
    /// The wait limit passed with no news; re-issue IDLE.
    TimedOut,
}

/// A calendar date for `SINCE` and `BEFORE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImapDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl ImapDate {
    /// The UTC calendar date of an epoch-seconds timestamp.
    pub fn from_epoch(secs: i64) -> Self {
        // Howard Hinnant's civil_from_days.
        let days = secs.div_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        Self { year, month, day }
    }

    /// `1-Jan-2026` form used in search keys.
    pub fn to_imap(self) -> String {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let idx = self.month.clamp(1, 12) as usize - 1;
        format!("{}-{}-{}", self.day, MONTHS[idx], self.year)
    }
}

/// `UID SEARCH` keys used by sync and server search (sections 6 and 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchKey {
    All,
    Since(ImapDate),
    Before(ImapDate),
    Text(String),
    From(String),
    To(String),
    Cc(String),
    Bcc(String),
    Subject(String),
    /// `HEADER name value` (RFC 3501): Undo without COPYUID finds a message by Message-ID.
    Header(String, String),
    Seen,
    Unseen,
    Flagged,
    Deleted,
    Larger(u64),
    Smaller(u64),
    Uid(UidSet),
    /// `UID n:*`: every UID from `n` up. Servers also return the highest UID when it is
    /// below `n`, so callers filter the result.
    UidFrom(u32),
    Not(Box<SearchKey>),
    Or(Box<SearchKey>, Box<SearchKey>),
    And(Vec<SearchKey>),
    /// `X-GM-RAW`, only on servers with `X-GM-EXT-1`.
    GmailRaw(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_parse_case_insensitive_and_imply_condstore() {
        let caps = Capabilities::parse([
            "IMAP4rev1",
            "idle",
            "Move",
            "QRESYNC",
            "APPENDLIMIT=1000",
            "AUTH=PLAIN",
            "x-gm-ext-1",
        ]);
        assert!(caps.idle && caps.move_ && caps.qresync && caps.condstore && caps.gmail);
        assert!(caps.auth_plain && !caps.auth_xoauth2 && !caps.uidplus);
        assert_eq!(caps.append_limit, Some(Some(1000)));
        assert_eq!(
            Capabilities::parse(["APPENDLIMIT"]).append_limit,
            Some(None)
        );
        assert_eq!(Capabilities::parse(["IMAP4rev1"]), Capabilities::default());
    }

    #[test]
    fn uid_set_compresses_ranges_and_chunks_at_500() {
        let set = UidSet::new([9, 1, 2, 3, 7, 10, 3, 0]);
        assert_eq!(set.to_imap(), "1:3,7,9:10");
        assert_eq!(set.len(), 6);
        assert!(set.contains(7) && !set.contains(8));

        let big = UidSet::new(1..=1201);
        let chunks = big.chunks();
        assert_eq!(
            chunks.iter().map(UidSet::len).collect::<Vec<_>>(),
            vec![500, 500, 201]
        );
        assert_eq!(chunks[1].to_imap(), "501:1000");
        assert!(UidSet::default().chunks().is_empty());
    }

    #[test]
    fn flags_round_trip() {
        for raw in [
            "\\Seen",
            "\\Flagged",
            "\\Answered",
            "\\Draft",
            "\\Deleted",
            "$Forwarded",
        ] {
            assert_eq!(Flag::parse(raw).as_imap(), raw);
        }
        assert_eq!(Flag::parse("\\SEEN"), Flag::Seen);
    }

    #[test]
    fn epoch_to_date() {
        assert_eq!(
            ImapDate::from_epoch(0),
            ImapDate {
                year: 1970,
                month: 1,
                day: 1
            }
        );
        // 2026-10-19T12:00:00Z
        assert_eq!(
            ImapDate::from_epoch(1_792_411_200),
            ImapDate {
                year: 2026,
                month: 10,
                day: 19
            }
        );
        // 2024-02-29T23:59:59Z
        assert_eq!(
            ImapDate::from_epoch(1_709_251_199),
            ImapDate {
                year: 2024,
                month: 2,
                day: 29
            }
        );
        assert_eq!(
            ImapDate {
                year: 2026,
                month: 1,
                day: 5
            }
            .to_imap(),
            "5-Jan-2026"
        );
    }

    #[test]
    fn special_use_attributes() {
        assert_eq!(
            SpecialUse::from_attribute("\\Trash"),
            Some(SpecialUse::Trash)
        );
        assert_eq!(SpecialUse::from_attribute("\\ALL"), Some(SpecialUse::All));
        assert_eq!(SpecialUse::from_attribute("\\HasNoChildren"), None);
    }
}
