#![forbid(unsafe_code)]
//! `search` (BUILD.md section 3): query parser, FTS5 queries, server search. May call
//! `store` and `imap`; never writes to the store.
//!
//! - [`parse`]: the section 10 syntax. Words are ANDed, `"phrases"`, `-term` negates, `OR`
//!   between terms, and the operators below. Unknown operators are plain words.
//! - [`local`]: a [`SearchSql`] over `messages AS m`: FTS5 for words, phrases and the
//!   sender, recipient and subject columns; plain predicates for the rest. The scope is the
//!   current account (or every account), all folders except Trash and Junk, unless the query
//!   names a folder.
//! - [`server_key`]: the part IMAP `UID SEARCH` can express; the rest runs locally on the
//!   fetched hits. On Gmail the query goes as `X-GM-RAW` instead.

use mail_imap::{ImapDate, SearchKey};
use mail_store::{AccountId, Folder, FolderId, SearchSql, SpecialUse, SqlParam};

/// Server search returns at most this many hits (section 10).
pub const SERVER_CAP: usize = 500;

const DAY: i64 = 86_400;

/// `is:` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Is {
    Unread,
    Read,
    Starred,
    Snoozed,
    Muted,
}

/// One search term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Word(String),
    Phrase(String),
    From(String),
    To(String),
    Cc(String),
    Bcc(String),
    Subject(String),
    /// `in:` and its alias `label:`: a folder path, name or role.
    In(String),
    Is(Is),
    HasAttachment,
    Filename(String),
    /// UTC midnight of the date; `before:` is exclusive, `after:` inclusive.
    Before(i64),
    After(i64),
    /// Seconds.
    OlderThan(i64),
    NewerThan(i64),
    /// Bytes.
    Larger(u64),
    Smaller(u64),
    Account(String),
}

/// A parsed query: terms ANDed, each a group of alternatives joined by `OR`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub groups: Vec<Vec<Unit>>,
}

/// A term, perhaps negated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub negated: bool,
    pub term: Term,
}

impl Query {
    fn terms(&self) -> impl Iterator<Item = &Unit> {
        self.groups.iter().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// Splits on whitespace, keeping `"quoted phrases"` (and `op:"quoted value"`) whole.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').to_string()
}

/// `YYYY-MM-DD` (also `YYYY/MM/DD`) as UTC midnight.
fn date(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split(['-', '/']).collect();
    let [y, m, d] = parts.as_slice() else {
        return None;
    };
    let (y, m, d): (i64, i64, i64) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&m) || d == 0 || d > 31 || !(1970..=9999).contains(&y) {
        return None;
    }
    Some(days_from_civil(y, m, d) * DAY)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `30d`, `6m`, `1y` as seconds (a month is 30 days, a year 365).
fn age(s: &str) -> Option<i64> {
    let (n, unit) = s.split_at(s.len().checked_sub(1)?);
    let n: i64 = n.parse().ok()?;
    let days = match unit.to_ascii_lowercase().as_str() {
        "d" => 1,
        "m" => 30,
        "y" => 365,
        _ => return None,
    };
    n.checked_mul(days * DAY)
}

/// `500k`, `1m`, or plain bytes.
fn size(s: &str) -> Option<u64> {
    let lower = s.to_ascii_lowercase();
    let (n, mult) = match lower.chars().last()? {
        'k' => (&lower[..lower.len() - 1], 1024),
        'm' => (&lower[..lower.len() - 1], 1024 * 1024),
        _ => (lower.as_str(), 1),
    };
    n.parse::<u64>().ok()?.checked_mul(mult)
}

fn term(token: &str) -> Term {
    if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
        return Term::Phrase(unquote(token));
    }
    let Some((op, value)) = token.split_once(':') else {
        return Term::Word(token.to_string());
    };
    let v = unquote(value);
    let word = || Term::Word(token.to_string());
    if v.is_empty() {
        return word();
    }
    match op.to_ascii_lowercase().as_str() {
        "from" => Term::From(v),
        "to" => Term::To(v),
        "cc" => Term::Cc(v),
        "bcc" => Term::Bcc(v),
        "subject" => Term::Subject(v),
        "in" | "label" => Term::In(v),
        "is" => match v.to_ascii_lowercase().as_str() {
            "unread" => Term::Is(Is::Unread),
            "read" => Term::Is(Is::Read),
            "starred" => Term::Is(Is::Starred),
            "snoozed" => Term::Is(Is::Snoozed),
            "muted" => Term::Is(Is::Muted),
            _ => word(),
        },
        "has" if v.eq_ignore_ascii_case("attachment") => Term::HasAttachment,
        "filename" => Term::Filename(v),
        "before" => date(&v).map_or_else(word, Term::Before),
        "after" => date(&v).map_or_else(word, Term::After),
        "older_than" => age(&v).map_or_else(word, Term::OlderThan),
        "newer_than" => age(&v).map_or_else(word, Term::NewerThan),
        "larger" => size(&v).map_or_else(word, Term::Larger),
        "smaller" => size(&v).map_or_else(word, Term::Smaller),
        "account" => Term::Account(v),
        _ => word(),
    }
}

/// Parses the section 10 syntax. Never fails: anything unrecognized is a plain word.
pub fn parse(text: &str) -> Query {
    let mut groups: Vec<Vec<Unit>> = Vec::new();
    let mut join = false;
    for token in tokens(text) {
        if token == "OR" {
            join = !groups.is_empty();
            continue;
        }
        let (negated, body) = match token.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => (true, rest),
            _ => (false, token.as_str()),
        };
        let unit = Unit {
            negated,
            term: term(body),
        };
        match (join, groups.last_mut()) {
            (true, Some(last)) => last.push(unit),
            _ => groups.push(vec![unit]),
        }
        join = false;
    }
    Query { groups }
}

/// What a local search needs to know about the user's accounts and folders.
#[derive(Debug, Clone)]
pub struct Context {
    /// Every account: id and address.
    pub accounts: Vec<(AccountId, String)>,
    /// Every folder of those accounts.
    pub folders: Vec<Folder>,
    /// The current account, or `None` for every account (the scope toggle).
    pub account: Option<AccountId>,
    /// UTC now, for `older_than:` and `newer_than:`.
    pub now: i64,
}

/// An FTS5 string: a quoted phrase, `*` for prefix matching.
fn fts_phrase(text: &str, prefix: bool) -> String {
    let inner = text.replace('"', "\"\"");
    if prefix {
        format!("\"{inner}\" *")
    } else {
        format!("\"{inner}\"")
    }
}

fn in_ids(ids: &[FolderId]) -> String {
    if ids.is_empty() {
        "NULL".into()
    } else {
        ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
    }
}

/// The folders an `in:` value names: path, last segment, or role, case-insensitive.
pub fn folders_named<'a>(folders: &'a [Folder], name: &str) -> Vec<&'a Folder> {
    let n = name.to_lowercase();
    let role = match n.as_str() {
        "inbox" => Some(SpecialUse::Inbox),
        "sent" => Some(SpecialUse::Sent),
        "drafts" | "draft" => Some(SpecialUse::Drafts),
        "trash" => Some(SpecialUse::Trash),
        "spam" | "junk" => Some(SpecialUse::Junk),
        "archive" => Some(SpecialUse::Archive),
        "all" | "anywhere" => Some(SpecialUse::All),
        _ => None,
    };
    folders
        .iter()
        .filter(|f| {
            let path = f.path.to_lowercase();
            let leaf = match f.delimiter.as_str() {
                "" => path.as_str(),
                d => path.rsplit(d).next().unwrap_or(&path),
            };
            path == n || leaf == n || (role.is_some() && f.special_use == role)
        })
        .collect()
}

struct Builder<'a> {
    ctx: &'a Context,
    /// Words and phrases so far, for prefix matching on the last token.
    params: Vec<SqlParam>,
}

impl Builder<'_> {
    fn text(&mut self, s: String) -> &'static str {
        self.params.push(SqlParam::Text(s));
        "?"
    }

    fn int(&mut self, i: i64) -> &'static str {
        self.params.push(SqlParam::Int(i));
        "?"
    }

    fn fts(&mut self, column: Option<&str>, value: &str, prefix: bool) -> String {
        let phrase = fts_phrase(value, prefix);
        let q = match column {
            Some(c) => format!("{c} : {phrase}"),
            None => phrase,
        };
        format!(
            "m.id IN (SELECT rowid FROM message_fts WHERE message_fts MATCH {})",
            self.text(q)
        )
    }

    fn folders_with(&self, role: SpecialUse) -> Vec<FolderId> {
        self.ctx
            .folders
            .iter()
            .filter(|f| f.special_use == Some(role))
            .map(|f| f.id)
            .collect()
    }

    fn unit(&mut self, t: &Term, last_word: bool) -> String {
        match t {
            Term::Word(w) => self.fts(None, w, last_word),
            Term::Phrase(p) => self.fts(None, p, false),
            Term::From(v) => self.fts(Some("sender"), v, true),
            Term::To(v) => {
                let narrow = self.fts(Some("recipients"), v, true);
                format!(
                    "({narrow} AND instr(lower(m.to_json), {}) > 0)",
                    self.text(v.to_lowercase())
                )
            }
            Term::Cc(v) => {
                let narrow = self.fts(Some("recipients"), v, true);
                format!(
                    "({narrow} AND instr(lower(m.cc_json), {}) > 0)",
                    self.text(v.to_lowercase())
                )
            }
            Term::Bcc(v) => {
                // Bcc is never in a header (section 7): local drafts and the outbox history
                // keep it (SPEC_GAPS G23). Sent and Drafts only.
                let drafts = in_ids(&self.folders_with(SpecialUse::Drafts));
                let sent = in_ids(&self.folders_with(SpecialUse::Sent));
                let a = self.text(v.to_lowercase());
                let b = self.text(v.to_lowercase());
                format!(
                    "((m.folder_id IN ({drafts}) AND m.uid IN (SELECT d.imap_uid FROM drafts AS d
                        WHERE d.account_id = m.account_id AND d.imap_uid IS NOT NULL
                          AND instr(lower(coalesce(json_extract(d.content_json, '$.bcc'), '')), {a}) > 0))
                      OR (m.folder_id IN ({sent}) AND m.message_id IN (SELECT json_extract(o.envelope_json, '$.message_id')
                        FROM outbox AS o WHERE o.account_id = m.account_id
                          AND instr(lower(coalesce(json_extract(o.envelope_json, '$.bcc'), '')), {b}) > 0)))"
                )
            }
            Term::Subject(v) => self.fts(Some("subject"), v, true),
            Term::In(name) => {
                let ids: Vec<FolderId> = folders_named(&self.ctx.folders, name)
                    .iter()
                    .map(|f| f.id)
                    .collect();
                format!("m.folder_id IN ({})", in_ids(&ids))
            }
            Term::Is(Is::Unread) => "(m.flags & 1) = 0".into(),
            Term::Is(Is::Read) => "(m.flags & 1) = 1".into(),
            Term::Is(Is::Starred) => "(m.flags & 2) = 2".into(),
            Term::Is(Is::Snoozed) => "m.id IN (SELECT message_id FROM snoozes)".into(),
            Term::Is(Is::Muted) => "m.thread_id IN (SELECT thread_id FROM muted_threads)".into(),
            Term::HasAttachment => "m.has_attach = 1".into(),
            Term::Filename(v) => format!(
                "m.id IN (SELECT message_id FROM attachments WHERE instr(lower(filename), {}) > 0)",
                self.text(v.to_lowercase())
            ),
            Term::Before(t) => format!("m.date_utc < {}", self.int(*t)),
            Term::After(t) => format!("m.date_utc >= {}", self.int(*t)),
            Term::OlderThan(s) => format!("m.date_utc < {}", self.int(self.ctx.now - s)),
            Term::NewerThan(s) => format!("m.date_utc >= {}", self.int(self.ctx.now - s)),
            Term::Larger(b) => format!(
                "m.size > {}",
                self.int(i64::try_from(*b).unwrap_or(i64::MAX))
            ),
            Term::Smaller(b) => format!(
                "m.size < {}",
                self.int(i64::try_from(*b).unwrap_or(i64::MAX))
            ),
            Term::Account(email) => {
                let ids: Vec<String> = self
                    .ctx
                    .accounts
                    .iter()
                    .filter(|(_, e)| e.eq_ignore_ascii_case(email))
                    .map(|(id, _)| id.to_string())
                    .collect();
                if ids.is_empty() {
                    "0".into()
                } else {
                    format!("m.account_id IN ({})", ids.join(","))
                }
            }
        }
    }
}

/// The accounts a query covers: those named by `account:`, else the current one, else all.
fn accounts_in_scope(q: &Query, ctx: &Context) -> Vec<AccountId> {
    let named: Vec<AccountId> = q
        .terms()
        .filter(|u| !u.negated)
        .filter_map(|u| match &u.term {
            Term::Account(e) => Some(e),
            _ => None,
        })
        .flat_map(|e| {
            ctx.accounts
                .iter()
                .filter(|(_, a)| a.eq_ignore_ascii_case(e))
                .map(|(id, _)| *id)
        })
        .collect();
    if !named.is_empty() {
        return named;
    }
    match ctx.account {
        Some(a) => vec![a],
        None => ctx.accounts.iter().map(|(id, _)| *id).collect(),
    }
}

/// Trash and Junk are out of scope unless the query names a folder with `in:`.
fn excluded_folders(q: &Query, ctx: &Context) -> Vec<FolderId> {
    let names_folder = q
        .terms()
        .any(|u| matches!(u.term, Term::In(_)) && !u.negated);
    if names_folder {
        return Vec::new();
    }
    ctx.folders
        .iter()
        .filter(|f| matches!(f.special_use, Some(SpecialUse::Trash | SpecialUse::Junk)))
        .map(|f| f.id)
        .collect()
}

/// The scope alone: accounts and folders, without the terms.
pub fn scope(q: &Query, ctx: &Context) -> SearchSql {
    let accounts = accounts_in_scope(q, ctx);
    let mut clause = format!(
        "m.account_id IN ({})",
        if accounts.is_empty() {
            "NULL".into()
        } else {
            accounts
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        }
    );
    let excluded = excluded_folders(q, ctx);
    if !excluded.is_empty() {
        clause.push_str(&format!(" AND m.folder_id NOT IN ({})", in_ids(&excluded)));
    }
    SearchSql {
        clause,
        params: Vec::new(),
    }
}

/// The local search for a parsed query (section 10).
pub fn local(q: &Query, ctx: &Context) -> SearchSql {
    let scope = scope(q, ctx);
    let last_word = q
        .terms()
        .enumerate()
        .filter(|(_, u)| matches!(u.term, Term::Word(_)))
        .map(|(i, _)| i)
        .last();
    let mut b = Builder {
        ctx,
        params: Vec::new(),
    };
    let mut parts = vec![scope.clause];
    let mut index = 0;
    for group in &q.groups {
        let mut alts = Vec::new();
        for u in group {
            let sql = b.unit(&u.term, Some(index) == last_word);
            index += 1;
            alts.push(if u.negated {
                format!("NOT ({sql})")
            } else {
                sql
            });
        }
        parts.push(format!("({})", alts.join(" OR ")));
    }
    SearchSql {
        clause: parts.join(" AND "),
        params: b.params,
    }
}

/// The messages in scope whose body is not cached, so not indexed: the UI offers Search
/// server too (section 10).
pub fn unindexed(q: &Query, ctx: &Context) -> SearchSql {
    let mut s = scope(q, ctx);
    s.clause.push_str(" AND m.body_state = 'none'");
    s
}

fn imap_term(t: &Term) -> Option<SearchKey> {
    Some(match t {
        Term::Word(w) | Term::Phrase(w) => SearchKey::Text(w.clone()),
        Term::From(v) => SearchKey::From(v.clone()),
        Term::To(v) => SearchKey::To(v.clone()),
        Term::Cc(v) => SearchKey::Cc(v.clone()),
        Term::Bcc(v) => SearchKey::Bcc(v.clone()),
        Term::Subject(v) => SearchKey::Subject(v.clone()),
        Term::Is(Is::Unread) => SearchKey::Unseen,
        Term::Is(Is::Read) => SearchKey::Seen,
        Term::Is(Is::Starred) => SearchKey::Flagged,
        Term::Before(t) => SearchKey::Before(ImapDate::from_epoch(*t)),
        Term::After(t) => SearchKey::Since(ImapDate::from_epoch(*t)),
        Term::Larger(b) => SearchKey::Larger(*b),
        Term::Smaller(b) => SearchKey::Smaller(*b),
        // Ages are relative: the caller has `now`; see `server_key`.
        _ => return None,
    })
}

/// The `UID SEARCH` key for the part of a query IMAP can express (section 10, Server
/// search). A term it cannot express is left out, so the server returns a superset, which
/// the local search then narrows. `None` when nothing can be sent.
pub fn server_key(q: &Query, now: i64) -> Option<SearchKey> {
    let mut keys = Vec::new();
    for group in &q.groups {
        let mut alts = Vec::new();
        let mut whole = true;
        for u in group {
            let key = match &u.term {
                Term::OlderThan(s) => Some(SearchKey::Before(ImapDate::from_epoch(now - s))),
                Term::NewerThan(s) => Some(SearchKey::Since(ImapDate::from_epoch(now - s))),
                t => imap_term(t),
            };
            match key {
                Some(k) if u.negated => alts.push(SearchKey::Not(Box::new(k))),
                Some(k) => alts.push(k),
                None => whole = false,
            }
        }
        // An OR group with a part the server cannot test is left out entirely.
        if !whole || alts.is_empty() {
            continue;
        }
        let mut alts = alts.into_iter();
        let first = alts.next()?;
        keys.push(alts.fold(first, |acc, k| SearchKey::Or(Box::new(acc), Box::new(k))));
    }
    match keys.len() {
        0 => None,
        1 => keys.pop(),
        _ => Some(SearchKey::And(keys)),
    }
}

/// The `X-GM-RAW` text for Gmail: the query without `account:` terms, which name this
/// app's accounts rather than anything Gmail knows.
pub fn gmail_raw(text: &str) -> String {
    tokens(text)
        .into_iter()
        .filter(|t| {
            let t = t.trim_start_matches('-').to_ascii_lowercase();
            !t.starts_with("account:")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The folders of `account` a server search covers: every synced or listed folder in
/// scope except Trash and Junk, or the folders `in:` names; `bcc:` covers Sent and Drafts.
pub fn server_folders(q: &Query, ctx: &Context, account: AccountId) -> Vec<FolderId> {
    let mine: Vec<&Folder> = ctx
        .folders
        .iter()
        .filter(|f| f.account_id == account)
        .collect();
    let named: Vec<FolderId> = q
        .terms()
        .filter(|u| !u.negated)
        .filter_map(|u| match &u.term {
            Term::In(n) => Some(n),
            _ => None,
        })
        .flat_map(|n| {
            folders_named(&ctx.folders, n)
                .into_iter()
                .filter(|f| f.account_id == account)
                .map(|f| f.id)
        })
        .collect();
    if !named.is_empty() {
        return named;
    }
    let bcc_only = q
        .terms()
        .any(|u| matches!(u.term, Term::Bcc(_)) && !u.negated);
    mine.iter()
        .filter(|f| {
            if bcc_only {
                matches!(f.special_use, Some(SpecialUse::Sent | SpecialUse::Drafts))
            } else {
                !matches!(
                    f.special_use,
                    Some(SpecialUse::Trash | SpecialUse::Junk | SpecialUse::All)
                )
            }
        })
        .map(|f| f.id)
        .collect()
}

/// The accounts a query covers (for server search).
pub fn accounts(q: &Query, ctx: &Context) -> Vec<AccountId> {
    accounts_in_scope(q, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(q: &Query) -> Vec<Vec<(bool, Term)>> {
        q.groups
            .iter()
            .map(|g| g.iter().map(|u| (u.negated, u.term.clone())).collect())
            .collect()
    }

    #[test]
    fn t30_query_parses() {
        let q = parse("from:alice has:attachment after:2026-01-01 budget");
        assert_eq!(
            units(&q),
            vec![
                vec![(false, Term::From("alice".into()))],
                vec![(false, Term::HasAttachment)],
                vec![(false, Term::After(1_767_225_600))],
                vec![(false, Term::Word("budget".into()))],
            ]
        );
    }

    #[test]
    fn t42_query_parses() {
        let q = parse("larger:1m older_than:30d -newsletter from:a OR from:b");
        assert_eq!(
            units(&q),
            vec![
                vec![(false, Term::Larger(1_048_576))],
                vec![(false, Term::OlderThan(30 * DAY))],
                vec![(true, Term::Word("newsletter".into()))],
                vec![
                    (false, Term::From("a".into())),
                    (false, Term::From("b".into()))
                ],
            ]
        );
    }

    #[test]
    fn phrases_unknown_operators_and_bad_values_are_words() {
        let q = parse("\"quarterly plan\" foo:bar is:weird before:2026-13-01 subject:\"big news\" label:Work -in:trash");
        assert_eq!(
            units(&q),
            vec![
                vec![(false, Term::Phrase("quarterly plan".into()))],
                vec![(false, Term::Word("foo:bar".into()))],
                vec![(false, Term::Word("is:weird".into()))],
                vec![(false, Term::Word("before:2026-13-01".into()))],
                vec![(false, Term::Subject("big news".into()))],
                vec![(false, Term::In("Work".into()))],
                vec![(true, Term::In("trash".into()))],
            ]
        );
        assert!(parse("   ").is_empty());
        assert_eq!(
            units(&parse("OR a")),
            vec![vec![(false, Term::Word("a".into()))]]
        );
        assert_eq!(size("500k"), Some(512_000));
        assert_eq!(age("2y"), Some(730 * DAY));
        assert_eq!(age("x"), None);
        assert_eq!(date("1970-01-01"), Some(0));
        assert_eq!(date("2024/02/29"), Some(1_709_164_800));
    }

    #[test]
    fn server_keys_keep_what_imap_can_express() {
        let now = 1_800_000_000;
        let q = parse("larger:1m older_than:30d -newsletter from:a OR from:b has:attachment");
        assert_eq!(
            server_key(&q, now),
            Some(SearchKey::And(vec![
                SearchKey::Larger(1_048_576),
                SearchKey::Before(ImapDate::from_epoch(now - 30 * DAY)),
                SearchKey::Not(Box::new(SearchKey::Text("newsletter".into()))),
                SearchKey::Or(
                    Box::new(SearchKey::From("a".into())),
                    Box::new(SearchKey::From("b".into()))
                ),
            ]))
        );
        assert_eq!(server_key(&parse("has:attachment is:muted"), now), None);
        assert_eq!(
            gmail_raw("from:a account:me@x.test -account:you@y budget"),
            "from:a budget"
        );
    }
}
