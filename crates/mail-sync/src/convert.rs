//! Server headers to store rows (section 6, step 4; section 12, missing headers).
//!
//! Headers are decoded by `mail-parser`, the spec's MIME parser (section 2). IDs are stored
//! without angle brackets. A missing Message-ID gets a synthetic one (section 10, rule 4);
//! a missing or bad Date, or one more than 24 h in the future, falls back to INTERNALDATE
//! (section 8). Missing From and Subject stay empty and the UI shows its placeholders.

use mail_imap::{BodyPart, FetchedHeader, Flag};
use mail_parser::{HeaderValue, MessageParser};
use mail_store::{Address, BodyState, Flags, NewAttachment, NewMessage};

/// A Date this far past now is ignored (section 8).
const MAX_FUTURE: i64 = 24 * 3600;

/// The store's flag bits for a set of IMAP flags (section 4).
pub(crate) fn flags(server: &[Flag]) -> Flags {
    let mut bits = 0;
    for f in server {
        bits |= match f {
            Flag::Seen => Flags::SEEN,
            Flag::Flagged => Flags::FLAGGED,
            Flag::Answered => Flags::ANSWERED,
            Flag::Draft => Flags::DRAFT,
            Flag::Deleted => Flags::DELETED,
            Flag::Keyword(k) if k.eq_ignore_ascii_case("$Forwarded") => Flags::FORWARDED,
            Flag::Keyword(_) => 0,
        };
    }
    Flags(bits)
}

pub(crate) fn new_message(
    h: &FetchedHeader,
    account_id: i64,
    folder_id: i64,
    now: i64,
    gmail: bool,
) -> NewMessage {
    let parsed = MessageParser::default().parse_headers(&h.headers);
    let header = parsed.as_ref();
    let message_id = header
        .and_then(|m| m.message_id())
        .map(bare_id)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| format!("synth:{account_id}:{folder_id}:{}@local", h.uid));
    let in_reply_to = header
        .and_then(|m| id_list(m.in_reply_to()).into_iter().next())
        .filter(|id| !id.is_empty());
    let refs = header
        .map(|m| id_list(m.references()).join(" "))
        .filter(|r| !r.is_empty());
    let date = header
        .and_then(|m| m.date())
        .filter(|d| d.is_valid())
        .map(|d| d.to_timestamp())
        .filter(|t| *t <= now + MAX_FUTURE)
        .unwrap_or(h.internal_date);
    let addresses = |a: Option<&mail_parser::Address<'_>>| -> Vec<Address> {
        a.map(|a| {
            a.iter()
                .filter_map(|addr| {
                    let address = addr.address().unwrap_or_default().trim().to_string();
                    let name = addr.name().unwrap_or_default().trim().to_string();
                    (!address.is_empty() || !name.is_empty()).then_some(Address {
                        name,
                        addr: address,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    };
    let from = addresses(header.and_then(|m| m.from()))
        .into_iter()
        .next()
        .unwrap_or_default();
    let attachments = attachments(&h.body);
    NewMessage {
        uid: h.uid,
        gm_msgid: h.gm_msgid.and_then(|v| i64::try_from(v).ok()),
        gm_thrid: h.gm_thrid.and_then(|v| i64::try_from(v).ok()),
        message_id,
        in_reply_to,
        refs,
        gm_labels: gmail.then(|| h.gm_labels.clone()),
        list_id: header
            .and_then(|m| m.header_raw("List-Id"))
            .and_then(list_id),
        list_unsubscribe: header
            .and_then(|m| m.header_raw("List-Unsubscribe"))
            .map(unfold)
            .filter(|v| !v.is_empty()),
        list_unsubscribe_post: header
            .and_then(|m| m.header_raw("List-Unsubscribe-Post"))
            .map(unfold)
            .filter(|v| !v.is_empty()),
        thread_id: 0,
        subject: header
            .and_then(|m| m.subject())
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        from,
        to: addresses(header.and_then(|m| m.to())),
        cc: addresses(header.and_then(|m| m.cc())),
        reply_to: addresses(header.and_then(|m| m.reply_to())),
        date_utc: date,
        internal_utc: h.internal_date,
        size: u64::from(h.size),
        flags: flags(&h.flags),
        has_attach: attachments.iter().any(|a| !a.is_inline),
        preview: String::new(),
        body_state: BodyState::None,
        attachments,
    }
}

/// The list identifier of a `List-Id` header (RFC 2919): the part in angle brackets,
/// lowercased; a header without brackets is taken whole.
fn list_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let id = match (raw.rfind('<'), raw.rfind('>')) {
        (Some(open), Some(close)) if open < close => &raw[open + 1..close],
        _ => raw,
    };
    let id = id.trim().to_lowercase();
    (!id.is_empty()).then_some(id)
}

/// A raw header value on one line, whitespace collapsed.
fn unfold(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bare_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

fn id_list(v: &HeaderValue<'_>) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => t.split_whitespace().map(bare_id).collect(),
        HeaderValue::TextList(list) => list.iter().map(|t| bare_id(t)).collect(),
        _ => Vec::new(),
    }
}

/// The parts shown as the message body: the first text/plain and the first text/html leaf
/// that is not an attachment.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TextParts<'a> {
    pub plain: Option<&'a BodyPart>,
    pub html: Option<&'a BodyPart>,
}

fn is_body_text(p: &BodyPart) -> bool {
    (p.mime == "text/plain" || p.mime == "text/html")
        && p.disposition.as_deref() != Some("attachment")
        && p.filename.is_none()
}

pub(crate) fn text_parts(root: &BodyPart) -> TextParts<'_> {
    let mut parts = TextParts::default();
    for leaf in root.leaves() {
        if !is_body_text(leaf) {
            continue;
        }
        if leaf.mime == "text/plain" && parts.plain.is_none() {
            parts.plain = Some(leaf);
        } else if leaf.mime == "text/html" && parts.html.is_none() {
            parts.html = Some(leaf);
        }
    }
    parts
}

/// Attachment metadata from BODYSTRUCTURE (section 8): every leaf that is not a body text
/// part. Inline parts (inline disposition or a Content-ID) are marked so; they do not count
/// toward `has_attach`.
pub(crate) fn attachments(root: &BodyPart) -> Vec<NewAttachment> {
    let body = text_parts(root);
    let is_body = |p: &BodyPart| {
        body.plain.is_some_and(|b| std::ptr::eq(b, p))
            || body.html.is_some_and(|b| std::ptr::eq(b, p))
    };
    root.leaves()
        .into_iter()
        .filter(|p| !is_body(p) && !p.mime.starts_with("multipart/"))
        .map(|p| NewAttachment {
            part_id: p.part_id.clone(),
            filename: p.filename.clone().unwrap_or_default(),
            mime: p.mime.clone(),
            size: p.size,
            content_id: p.content_id.clone(),
            is_inline: p.disposition.as_deref() == Some("inline") || p.content_id.is_some(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_ids() {
        assert_eq!(
            super::list_id("Weekly News <News.Example.COM>"),
            Some("news.example.com".to_string())
        );
        assert_eq!(super::list_id(" plain.list "), Some("plain.list".into()));
        assert_eq!(super::list_id("  "), None);
    }

    use super::*;

    fn header(raw: &str) -> FetchedHeader {
        FetchedHeader {
            uid: 42,
            flags: vec![Flag::Seen, Flag::Keyword("$Forwarded".into())],
            internal_date: 1_792_000_000,
            size: 1234,
            headers: raw.as_bytes().to_vec(),
            body: BodyPart::text_plain(10),
            modseq: None,
            gm_msgid: Some(11),
            gm_thrid: Some(22),
            gm_labels: vec!["\\Inbox".into()],
        }
    }

    const NOW: i64 = 1_792_411_200;

    #[test]
    fn decodes_headers_and_addresses() {
        let h = header(
            "From: =?utf-8?q?J=C3=BCrgen?= <jurgen@example.test>\r\n\
             To: Bob <bob@example.test>, carol@example.test\r\n\
             Cc: Team: dave@example.test;\r\n\
             Subject: =?utf-8?b?R3LDvMOfZQ==?=\r\n\
             Message-ID: <abc@example.test>\r\n\
             In-Reply-To: <parent@example.test>\r\n\
             References: <root@example.test> <parent@example.test>\r\n\
             Date: Mon, 19 Oct 2026 10:00:00 +0000\r\n\r\n",
        );
        let m = new_message(&h, 1, 2, NOW, true);
        assert_eq!(m.from.name, "Jürgen");
        assert_eq!(m.from.addr, "jurgen@example.test");
        assert_eq!(m.to.len(), 2);
        assert_eq!(m.to[1].addr, "carol@example.test");
        assert_eq!(m.cc[0].addr, "dave@example.test");
        assert_eq!(m.subject, "Grüße");
        assert_eq!(m.message_id, "abc@example.test");
        assert_eq!(m.in_reply_to.as_deref(), Some("parent@example.test"));
        assert_eq!(
            m.refs.as_deref(),
            Some("root@example.test parent@example.test")
        );
        assert_eq!(m.date_utc, 1_792_404_000);
        assert_eq!(m.internal_utc, 1_792_000_000);
        assert_eq!(m.flags, Flags(Flags::SEEN | Flags::FORWARDED));
        assert_eq!((m.gm_msgid, m.gm_thrid), (Some(11), Some(22)));
        assert_eq!(m.gm_labels, Some(vec!["\\Inbox".to_string()]));
        assert_eq!(m.body_state, BodyState::None);
    }

    #[test]
    fn missing_or_bad_headers_fall_back() {
        let m = new_message(&header("X-Other: 1\r\n\r\n"), 1, 2, NOW, false);
        assert_eq!(m.message_id, "synth:1:2:42@local");
        assert_eq!(m.date_utc, 1_792_000_000, "no Date: INTERNALDATE");
        assert_eq!(m.from, Address::default());
        assert!(m.subject.is_empty() && m.in_reply_to.is_none() && m.refs.is_none());
        assert_eq!(m.gm_labels, None);

        let future = new_message(
            &header("Date: Fri, 1 Jan 2027 00:00:00 +0000\r\n\r\n"),
            1,
            2,
            NOW,
            false,
        );
        assert_eq!(future.date_utc, 1_792_000_000, "over 24 h ahead: ignored");
        let garbage = new_message(&header("Date: yesterday-ish\r\n\r\n"), 1, 2, NOW, false);
        assert_eq!(garbage.date_utc, 1_792_000_000);
        let empty = new_message(&header(""), 1, 2, NOW, false);
        assert_eq!(empty.message_id, "synth:1:2:42@local");
    }

    fn leaf(
        id: &str,
        mime: &str,
        disposition: Option<&str>,
        filename: Option<&str>,
        cid: Option<&str>,
    ) -> BodyPart {
        BodyPart {
            part_id: id.into(),
            mime: mime.into(),
            charset: None,
            encoding: Some("base64".into()),
            content_id: cid.map(str::to_string),
            disposition: disposition.map(str::to_string),
            filename: filename.map(str::to_string),
            size: 100,
            flowed: false,
            delsp: false,
            children: Vec::new(),
        }
    }

    #[test]
    fn body_parts_and_attachments() {
        let alternative = BodyPart {
            part_id: "1".into(),
            mime: "multipart/alternative".into(),
            children: vec![
                leaf("1.1", "text/plain", None, None, None),
                leaf("1.2", "text/html", None, None, None),
            ],
            ..leaf("1", "multipart/alternative", None, None, None)
        };
        let root = BodyPart {
            part_id: String::new(),
            mime: "multipart/mixed".into(),
            children: vec![
                alternative,
                leaf(
                    "2",
                    "image/png",
                    Some("inline"),
                    Some("logo.png"),
                    Some("logo@x"),
                ),
                leaf(
                    "3",
                    "text/plain",
                    Some("attachment"),
                    Some("notes.txt"),
                    None,
                ),
                leaf("4", "application/pdf", None, Some("r.pdf"), None),
            ],
            ..leaf("", "multipart/mixed", None, None, None)
        };
        let parts = text_parts(&root);
        assert_eq!(parts.plain.map(|p| p.part_id.as_str()), Some("1.1"));
        assert_eq!(parts.html.map(|p| p.part_id.as_str()), Some("1.2"));
        let att = attachments(&root);
        let ids: Vec<&str> = att.iter().map(|a| a.part_id.as_str()).collect();
        assert_eq!(ids, vec!["2", "3", "4"]);
        assert!(att[0].is_inline && !att[1].is_inline && !att[2].is_inline);
        assert_eq!(att[1].filename, "notes.txt");

        let only_inline = BodyPart {
            children: vec![
                leaf("1", "text/html", None, None, None),
                leaf("2", "image/png", None, None, Some("cid1")),
            ],
            ..leaf("", "multipart/related", None, None, None)
        };
        let h = FetchedHeader {
            body: only_inline,
            ..header("\r\n")
        };
        assert!(
            !new_message(&h, 1, 2, NOW, false).has_attach,
            "inline images only"
        );
    }
}
