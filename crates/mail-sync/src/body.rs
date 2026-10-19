//! Body prefetch (section 6, step 8) and list previews (section 8).
//!
//! The prefetcher fetches the text and HTML body parts of a message whole, each at most
//! 1 MB. A message with a larger text part is not prefetched (section 12, Very large
//! message); opening it later fetches a capped part (`body_state` text, SPEC_GAPS G35).
//! HTML-only mail gets generated plain text for previews and search (section 8).

use mail_imap::{BodyPart, ImapError, ImapTransport};
use mail_parser::decoders::base64::base64_decode;
use mail_parser::decoders::charsets::map::charset_decoder;
use mail_parser::decoders::html::html_to_text;
use mail_parser::decoders::quoted_printable::quoted_printable_decode;
use mail_store::{Body, BodyState};

use crate::convert::text_parts;

/// The largest text part prefetched (section 6, step 8).
pub const PART_LIMIT: u64 = 1024 * 1024;
/// Preview length in characters (section 8).
pub const PREVIEW_CHARS: usize = 140;

/// Why a body is fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// Background prefetch: a message with a text part over 1 MB is skipped (G39).
    Prefetch,
    /// The user opened it: parts over 1 MB are cut there, and the state is `text` (G35).
    Open,
}

/// Fetches and decodes a message's body parts. `None` when prefetch skips the message.
pub(crate) async fn fetch_body<T: ImapTransport>(
    t: &mut T,
    uid: u32,
    root: &BodyPart,
    now: i64,
    purpose: Purpose,
) -> Result<Option<(Body, BodyState)>, ImapError> {
    let parts = text_parts(root);
    let oversize = [parts.plain, parts.html]
        .iter()
        .flatten()
        .any(|p| p.size > PART_LIMIT);
    if oversize && purpose == Purpose::Prefetch {
        return Ok(None);
    }
    let plain = fetch_text(t, uid, parts.plain).await?;
    let html = fetch_text(t, uid, parts.html).await?;
    let text_plain = if parts.plain.is_none() && !html.is_empty() {
        html_to_text(&html)
    } else {
        plain
    };
    Ok(Some((
        Body {
            text_plain,
            text_html: html,
            fetched_at: now,
        },
        if oversize {
            BodyState::Text
        } else {
            BodyState::Full
        },
    )))
}

async fn fetch_text<T: ImapTransport>(
    t: &mut T,
    uid: u32,
    part: Option<&BodyPart>,
) -> Result<String, ImapError> {
    match part {
        Some(p) => {
            let cut = p.size > PART_LIMIT;
            let raw = t
                .uid_fetch_part(uid, &p.part_id, cut.then_some(PART_LIMIT))
                .await?;
            Ok(decode_part(&raw, p))
        }
        None => Ok(String::new()),
    }
}

/// Transfer decoding, then charset decoding, then `format=flowed` (section 8). Unknown or
/// invalid charset: UTF-8, then Windows-1252, then lossy UTF-8. Never fails.
pub(crate) fn decode_part(raw: &[u8], part: &BodyPart) -> String {
    let bytes = decode_transfer(raw, part.encoding.as_deref());
    let text = decode_text(&bytes, part.charset.as_deref());
    if part.flowed && part.mime == "text/plain" {
        unflow(&text, part.delsp)
    } else {
        text
    }
}

/// Undoes a content transfer encoding (base64 or quoted-printable). Anything else, and
/// data that does not decode, is returned as it is.
pub(crate) fn decode_transfer(raw: &[u8], encoding: Option<&str>) -> Vec<u8> {
    match encoding.map(str::to_ascii_lowercase).as_deref() {
        Some("base64") => {
            let mut compact: Vec<u8> = raw
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            // A part cut at the size cap may end mid-quantum.
            compact.truncate(compact.len() / 4 * 4);
            base64_decode(&compact).unwrap_or_else(|| raw.to_vec())
        }
        Some("quoted-printable") => quoted_printable_decode(raw).unwrap_or_else(|| raw.to_vec()),
        _ => raw.to_vec(),
    }
}

/// RFC 3676: joins soft-broken lines (a line ending in a space continues on the next line
/// of the same quote depth), removes space-stuffing, and with `delsp=yes` drops the space
/// that marked the break. The signature separator `-- ` is never joined. Quoted lines keep
/// one `>` per level followed by a space.
pub(crate) fn unflow(text: &str, delsp: bool) -> String {
    let mut out = String::with_capacity(text.len());
    // The paragraph being joined and its quote depth.
    let mut open: Option<(usize, String)> = None;
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    for raw in lines {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let depth = line.chars().take_while(|c| *c == '>').count();
        let mut content = &line[depth..];
        content = content.strip_prefix(' ').unwrap_or(content);
        let soft = content.ends_with(' ') && content != "-- ";
        if let Some((d, para)) = open.as_mut() {
            if *d != depth {
                flush(&mut out, *d, para);
                open = None;
            }
        }
        let piece = if soft && delsp {
            &content[..content.len() - 1]
        } else {
            content
        };
        match open.as_mut() {
            Some((_, para)) => para.push_str(piece),
            None => open = Some((depth, piece.to_string())),
        }
        if !soft {
            if let Some((d, para)) = open.take() {
                flush(&mut out, d, &para);
            }
        }
    }
    if let Some((d, para)) = open {
        flush(&mut out, d, &para);
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

fn flush(out: &mut String, depth: usize, para: &str) {
    if depth > 0 {
        out.extend(std::iter::repeat_n('>', depth));
        out.push(' ');
    }
    out.push_str(para);
    out.push('\n');
}

fn decode_text(bytes: &[u8], charset: Option<&str>) -> String {
    let named = charset
        .filter(|c| !c.eq_ignore_ascii_case("utf-8") && !c.eq_ignore_ascii_case("us-ascii"))
        .and_then(|c| charset_decoder(c.as_bytes()));
    if let Some(decode) = named {
        return decode(bytes);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    match charset_decoder(b"windows-1252") {
        Some(decode) => decode(bytes),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The list preview: the first 140 characters of plain text, whitespace collapsed, quoted
/// lines (`>`) and everything after the `-- ` signature line dropped (section 8).
pub fn preview(text: &str) -> String {
    let mut words: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim_end_matches('\r') == "-- " {
            break;
        }
        if line.trim_start().starts_with('>') {
            continue;
        }
        words.extend(line.split_whitespace());
    }
    words.join(" ").chars().take(PREVIEW_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(encoding: &str, charset: Option<&str>) -> BodyPart {
        BodyPart {
            encoding: Some(encoding.into()),
            charset: charset.map(str::to_string),
            ..BodyPart::text_plain(10)
        }
    }

    #[test]
    fn decodes_transfer_encodings_and_charsets() {
        assert_eq!(
            decode_part(b"SGVs\r\nbG8=", &part("base64", Some("utf-8"))),
            "Hello"
        );
        assert_eq!(
            decode_part(
                b"Caf=C3=A9 =\r\nau lait",
                &part("quoted-printable", Some("UTF-8"))
            ),
            "Café au lait"
        );
        assert_eq!(
            decode_part(b"Caf\xe9", &part("8bit", Some("iso-8859-1"))),
            "Café"
        );
        // A bogus label falls back: valid UTF-8 first, then Windows-1252.
        assert_eq!(
            decode_part("Grüße".as_bytes(), &part("8bit", Some("x-bogus"))),
            "Grüße"
        );
        assert_eq!(
            decode_part(b"\x93quoted\x94", &part("8bit", Some("x-bogus"))),
            "\u{201c}quoted\u{201d}"
        );
        assert_eq!(decode_part(b"plain", &part("7bit", None)), "plain");
        // Shift_JIS through mail-parser's multi-byte decoders.
        assert_eq!(
            decode_part(b"\x93\xfa\x96{", &part("8bit", Some("shift_jis"))),
            "日本"
        );
    }

    #[test]
    fn format_flowed_is_joined() {
        let text = "This is a \r\nflowed para.\r\n\r\n> quoted \r\n> soft\r\n>> deep\r\n -- not sig\r\n-- \r\nSig";
        assert_eq!(
            unflow(text, false),
            "This is a flowed para.\n\n> quoted soft\n>> deep\n-- not sig\n-- \nSig"
        );
        assert_eq!(unflow("Hel \r\nlo\r\n", true), "Hello\n");
        let flowed = BodyPart {
            flowed: true,
            ..part("7bit", Some("utf-8"))
        };
        assert_eq!(decode_part(b"a \r\nb", &flowed), "a b");
        assert_eq!(decode_transfer(b"aGk=", Some("BASE64")), b"hi");
        assert_eq!(decode_transfer(b"x=3Dy", Some("quoted-printable")), b"x=y");
        assert_eq!(decode_transfer(b"raw", Some("binary")), b"raw");
    }

    #[test]
    fn previews_skip_quotes_and_signatures() {
        let text = "Hi Bob,\r\n\r\n  Lunch   at\tnoon?\r\n> old quoted text\r\n>> deeper\r\nThanks\r\n-- \r\nAlice\r\nSig line";
        assert_eq!(preview(text), "Hi Bob, Lunch at noon? Thanks");
        let long = "word ".repeat(100);
        assert_eq!(preview(&long).chars().count(), PREVIEW_CHARS);
        assert_eq!(
            preview("é".repeat(200).as_str()).chars().count(),
            PREVIEW_CHARS
        );
        assert_eq!(preview(""), "");
    }
}
