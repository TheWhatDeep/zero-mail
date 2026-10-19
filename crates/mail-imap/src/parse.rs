//! Conversions from `imap-proto` responses to this crate's types.

use std::borrow::Cow;

use async_imap::imap_proto::rfc4315::UidSetMember;
use async_imap::imap_proto::{
    BodyContentCommon, BodyContentSinglePart, BodyStructure, Capability, ContentEncoding,
    NameAttribute,
};

use crate::types::{BodyPart, Capabilities, Flag, SpecialUse};

/// Capabilities from a `CAPABILITY` response or response code.
pub(crate) fn capabilities(caps: &[Capability<'_>]) -> Capabilities {
    let atoms: Vec<String> = caps
        .iter()
        .map(|c| match c {
            Capability::Imap4rev1 => "IMAP4rev1".to_string(),
            Capability::Auth(mech) => format!("AUTH={mech}"),
            Capability::Atom(atom) => atom.to_string(),
        })
        .collect();
    Capabilities::parse(atoms.iter().map(String::as_str))
}

/// Flags as the client models them. `\Recent` is session state, not a message flag.
pub(crate) fn flags(raw: &[Cow<'_, str>]) -> Vec<Flag> {
    raw.iter()
        .filter(|f| !f.eq_ignore_ascii_case("\\Recent"))
        .map(|f| Flag::parse(f))
        .collect()
}

/// A `LIST` attribute as text, and its special-use role if it has one (RFC 6154).
pub(crate) fn name_attribute(attr: &NameAttribute<'_>) -> (String, Option<SpecialUse>) {
    let (text, role) = match attr {
        NameAttribute::NoInferiors => ("\\Noinferiors", None),
        NameAttribute::NoSelect => ("\\Noselect", None),
        NameAttribute::Marked => ("\\Marked", None),
        NameAttribute::Unmarked => ("\\Unmarked", None),
        NameAttribute::All => ("\\All", Some(SpecialUse::All)),
        NameAttribute::Archive => ("\\Archive", Some(SpecialUse::Archive)),
        NameAttribute::Drafts => ("\\Drafts", Some(SpecialUse::Drafts)),
        NameAttribute::Flagged => ("\\Flagged", None),
        NameAttribute::Junk => ("\\Junk", Some(SpecialUse::Junk)),
        NameAttribute::Sent => ("\\Sent", Some(SpecialUse::Sent)),
        NameAttribute::Trash => ("\\Trash", Some(SpecialUse::Trash)),
        NameAttribute::Extension(other) => {
            let role = SpecialUse::from_attribute(other);
            return (other.to_string(), role);
        }
        _ => ("", None),
    };
    (text.to_string(), role)
}

/// Expands a UIDPLUS uid-set. Ranges in COPYUID and APPENDUID mirror a source set of at most
/// 500 UIDs (section 9); anything larger is treated as malformed.
pub(crate) fn uid_members(members: &[UidSetMember]) -> Option<Vec<u32>> {
    const LIMIT: usize = 100_000;
    let mut out = Vec::new();
    for m in members {
        match m {
            UidSetMember::Uid(uid) => out.push(*uid),
            UidSetMember::UidRange(range) => {
                let (lo, hi) = (
                    *range.start().min(range.end()),
                    *range.start().max(range.end()),
                );
                if (hi - lo) as usize >= LIMIT {
                    return None;
                }
                out.extend(lo..=hi);
            }
        }
        if out.len() > LIMIT {
            return None;
        }
    }
    Some(out)
}

/// `[LIMIT]`, `[UNAVAILABLE]` or a provider's command or bandwidth message (section 6).
pub(crate) fn is_throttle(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "[limit]",
        "[unavailable]",
        "bandwidth",
        "too many simultaneous connections",
    ]
    .iter()
    .any(|needle| t.contains(needle))
}

/// INTERNALDATE, for example `17-Jul-1996 02:44:25 -0700`, as UTC epoch seconds.
pub(crate) fn internal_date(text: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let mut parts = text.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let zone = parts.next()?;
    let mut d = date.split('-');
    let day: u32 = d.next()?.parse().ok()?;
    let month_name = d.next()?.to_ascii_lowercase();
    let month = MONTHS.iter().position(|m| *m == month_name)? as u32 + 1;
    let year: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let m: i64 = t.next()?.parse().ok()?;
    let s: i64 = t.next()?.parse().ok()?;
    let (sign, digits) = match zone.split_at_checked(1)? {
        ("+", rest) => (1, rest),
        ("-", rest) => (-1, rest),
        _ => return None,
    };
    if digits.len() != 4 || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    let zh: i64 = digits.get(..2)?.parse().ok()?;
    let zm: i64 = digits.get(2..)?.parse().ok()?;
    let offset = sign * (zh * 3600 + zm * 60);
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + h * 3600 + m * 60 + s - offset)
}

/// Howard Hinnant's days_from_civil: days since 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A `BODYSTRUCTURE` as a [`BodyPart`] tree with IMAP part specifiers. A non-multipart root
/// is part `1`; an embedded `message/rfc822` is one leaf (an attachment), not descended.
pub(crate) fn body_part(bs: &BodyStructure<'_>) -> BodyPart {
    match bs {
        BodyStructure::Multipart { .. } => node(bs, String::new()),
        _ => node(bs, "1".to_string()),
    }
}

fn node(bs: &BodyStructure<'_>, part_id: String) -> BodyPart {
    match bs {
        BodyStructure::Multipart { common, bodies, .. } => {
            let children = bodies
                .iter()
                .enumerate()
                .map(|(i, child)| {
                    let id = if part_id.is_empty() {
                        (i + 1).to_string()
                    } else {
                        format!("{part_id}.{}", i + 1)
                    };
                    node(child, id)
                })
                .collect();
            let mut part = leaf(common, None, part_id);
            part.children = children;
            part
        }
        BodyStructure::Basic { common, other, .. }
        | BodyStructure::Text { common, other, .. }
        | BodyStructure::Message { common, other, .. } => leaf(common, Some(other), part_id),
    }
}

fn leaf(
    common: &BodyContentCommon<'_>,
    single: Option<&BodyContentSinglePart<'_>>,
    part_id: String,
) -> BodyPart {
    let mime = format!("{}/{}", common.ty.ty, common.ty.subtype).to_ascii_lowercase();
    let ct_param = |name: &str| param(&common.ty.params, name);
    let disposition = common.disposition.as_ref();
    let filename = disposition
        .and_then(|d| param(&d.params, "filename").or_else(|| param(&d.params, "filename*")))
        .or_else(|| ct_param("name"));
    BodyPart {
        part_id,
        mime,
        charset: ct_param("charset"),
        encoding: single.map(|s| encoding(&s.transfer_encoding)),
        content_id: single.and_then(|s| s.id.as_ref()).map(|id| {
            id.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        }),
        disposition: disposition.map(|d| d.ty.to_ascii_lowercase()),
        filename,
        size: single.map_or(0, |s| u64::from(s.octets)),
        children: Vec::new(),
    }
}

fn param(params: &Option<Vec<(Cow<'_, str>, Cow<'_, str>)>>, name: &str) -> Option<String> {
    params
        .as_ref()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.to_string())
}

fn encoding(e: &ContentEncoding<'_>) -> String {
    match e {
        ContentEncoding::SevenBit => "7bit".into(),
        ContentEncoding::EightBit => "8bit".into(),
        ContentEncoding::Binary => "binary".into(),
        ContentEncoding::Base64 => "base64".into(),
        ContentEncoding::QuotedPrintable => "quoted-printable".into(),
        ContentEncoding::Other(other) => other.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_imap::imap_proto::{AttributeValue, Response};

    #[test]
    fn internal_dates() {
        assert_eq!(
            internal_date("17-Jul-1996 02:44:25 -0700"),
            Some(837_596_665)
        );
        assert_eq!(internal_date(" 1-Jan-1970 00:00:00 +0000"), Some(0));
        assert_eq!(
            internal_date("19-Oct-2026 12:00:00 +0000"),
            Some(1_792_411_200)
        );
        assert_eq!(
            internal_date("29-Feb-2024 23:59:59 +0000"),
            Some(1_709_251_199)
        );
        assert_eq!(
            internal_date("19-Oct-2026 14:00:00 +0200"),
            Some(1_792_411_200)
        );
        assert_eq!(internal_date("nonsense"), None);
        assert_eq!(internal_date("32-Jan-2026 00:00:00 +0000"), None);
        assert_eq!(internal_date("1-Foo-2026 00:00:00 +0000"), None);
    }

    #[test]
    fn throttle_messages() {
        assert!(is_throttle("[LIMIT] Too many commands"));
        assert!(is_throttle("[UNAVAILABLE] try later"));
        assert!(is_throttle("Account exceeded command or bandwidth limits."));
        assert!(is_throttle(
            "[ALERT] Too many simultaneous connections. (Failure)"
        ));
        assert!(!is_throttle("[AUTHENTICATIONFAILED] Invalid credentials"));
    }

    #[test]
    fn uid_sets_expand_and_cap() {
        // IMAP allows a range written high to low, such as 9:7.
        let (hi, lo) = (9, 7);
        let set = [UidSetMember::Uid(4), UidSetMember::UidRange(hi..=lo)];
        assert_eq!(uid_members(&set), Some(vec![4, 7, 8, 9]));
        assert_eq!(uid_members(&[UidSetMember::UidRange(1..=u32::MAX)]), None);
    }

    fn parse_bodystructure(raw: &[u8]) -> BodyPart {
        let (_, resp) = Response::parse(raw).unwrap();
        let attrs = match resp {
            Response::Fetch(_, attrs) => attrs,
            _ => Vec::new(),
        };
        attrs
            .iter()
            .find_map(|a| match a {
                AttributeValue::BodyStructure(bs) => Some(body_part(bs)),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn bodystructure_single_part() {
        let part = parse_bodystructure(
            b"* 1 FETCH (BODYSTRUCTURE (\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\") NIL NIL \"7BIT\" 42 3 NIL NIL NIL NIL))\r\n",
        );
        assert_eq!(part.part_id, "1");
        assert_eq!(part.mime, "text/plain");
        assert_eq!(part.charset.as_deref(), Some("UTF-8"));
        assert_eq!(part.encoding.as_deref(), Some("7bit"));
        assert_eq!(part.size, 42);
        assert!(part.children.is_empty());
    }

    #[test]
    fn bodystructure_nested_multipart_with_attachment() {
        let raw = b"* 1 FETCH (BODYSTRUCTURE (((\"TEXT\" \"PLAIN\" (\"CHARSET\" \"utf-8\") NIL NIL \"QUOTED-PRINTABLE\" 10 1 NIL NIL NIL NIL)(\"TEXT\" \"HTML\" (\"CHARSET\" \"utf-8\") NIL NIL \"BASE64\" 20 1 NIL NIL NIL NIL) \"ALTERNATIVE\" (\"BOUNDARY\" \"b1\") NIL NIL NIL)(\"IMAGE\" \"PNG\" (\"NAME\" \"logo.png\") \"<logo@x>\" NIL \"BASE64\" 300 NIL (\"INLINE\" (\"FILENAME\" \"logo.png\")) NIL NIL)(\"APPLICATION\" \"PDF\" NIL NIL NIL \"BASE64\" 4000 NIL (\"ATTACHMENT\" (\"FILENAME\" \"r.pdf\")) NIL NIL) \"MIXED\" (\"BOUNDARY\" \"b0\") NIL NIL NIL))\r\n";
        let root = parse_bodystructure(raw);
        assert_eq!(root.part_id, "");
        assert_eq!(root.mime, "multipart/mixed");
        let leaves = root.leaves();
        let ids: Vec<&str> = leaves.iter().map(|p| p.part_id.as_str()).collect();
        assert_eq!(ids, vec!["1.1", "1.2", "2", "3"]);
        assert_eq!(leaves[1].mime, "text/html");
        assert_eq!(leaves[2].content_id.as_deref(), Some("logo@x"));
        assert_eq!(leaves[2].disposition.as_deref(), Some("inline"));
        assert_eq!(leaves[3].filename.as_deref(), Some("r.pdf"));
        assert_eq!(leaves[3].size, 4000);
    }
}
