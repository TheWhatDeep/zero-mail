//! The outgoing MIME build (section 7). Headers set by the client: Message-ID
//! (`<uuid@from-domain>`), Date, From, To, Cc, Reply-To if set, Subject, In-Reply-To and
//! References for replies, MIME-Version and Content-Type. No X-Mailer, and never a Bcc
//! header: Bcc addresses go only into the envelope. The body is `multipart/alternative`
//! (plain and HTML) or plain only; attachments make it `multipart/mixed`. Non-ASCII headers
//! use RFC 2047, text is UTF-8 quoted-printable, attachments base64.

use std::time::SystemTime;

use lettre::message::header::{ContentTransferEncoding, ContentType};
use lettre::message::{Attachment, Body, Mailbox, MultiPart, SinglePart};
use lettre::Message;

use crate::SmtpError;

/// A display name and an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxSpec {
    pub name: String,
    pub addr: String,
}

/// One attached file, already read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A message to build.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutgoingMessage {
    pub from: Option<MailboxSpec>,
    pub to: Vec<MailboxSpec>,
    pub cc: Vec<MailboxSpec>,
    /// Envelope only (section 7).
    pub bcc: Vec<MailboxSpec>,
    pub reply_to: Option<String>,
    pub subject: String,
    pub text: String,
    /// `None`: plain only (the composer's plain toggle).
    pub html: Option<String>,
    pub attachments: Vec<OutgoingAttachment>,
    /// The raw original for forward-as-attachment (`message/rfc822`).
    pub forwarded: Option<Vec<u8>>,
    /// Bare Message-IDs, without angle brackets.
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Reuse an ID (draft versions keep theirs); `None` makes a new one.
    pub message_id: Option<String>,
    /// UTC epoch seconds for the Date header.
    pub date: i64,
}

/// The raw message and the envelope it goes with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltMessage {
    pub raw: Vec<u8>,
    /// Bare Message-ID.
    pub message_id: String,
    pub envelope: crate::Envelope,
}

fn mailbox(m: &MailboxSpec) -> Result<Mailbox, SmtpError> {
    let addr = m
        .addr
        .trim()
        .parse()
        .map_err(|_| SmtpError::Rejected(format!("invalid address {}", m.addr)))?;
    let name = m.name.trim();
    Ok(Mailbox::new(
        (!name.is_empty()).then(|| name.to_string()),
        addr,
    ))
}

/// The domain of an address, for the Message-ID.
fn domain(addr: &str) -> &str {
    addr.rsplit_once('@').map_or("localhost", |(_, d)| d)
}

fn text_part(content_type: ContentType, text: &str) -> Result<SinglePart, SmtpError> {
    let body = Body::new_with_encoding(text.to_string(), ContentTransferEncoding::QuotedPrintable)
        .map_err(|_| SmtpError::Rejected("text could not be encoded".into()))?;
    Ok(SinglePart::builder().header(content_type).body(body))
}

/// An embedded original: 7bit when it is plain ASCII with short lines, else 8bit
/// (RFC 2046 forbids base64 here).
fn forwarded_part(raw: &[u8]) -> Result<SinglePart, SmtpError> {
    let short = raw.split(|b| *b == b'\n').all(|l| l.len() <= 998);
    let encoding = if raw.is_ascii() && short {
        ContentTransferEncoding::SevenBit
    } else {
        ContentTransferEncoding::EightBit
    };
    let content_type = ContentType::parse("message/rfc822")
        .map_err(|_| SmtpError::Rejected("content type".into()))?;
    Ok(SinglePart::builder()
        .header(content_type)
        .header(lettre::message::header::ContentDisposition::attachment(
            "forwarded.eml",
        ))
        .header(encoding)
        .body(raw.to_vec()))
}

impl OutgoingMessage {
    /// Builds the raw message (CRLF line ends) and its envelope.
    pub fn build(&self) -> Result<BuiltMessage, SmtpError> {
        let from = self
            .from
            .as_ref()
            .ok_or_else(|| SmtpError::Rejected("no sender".into()))?;
        let message_id = self
            .message_id
            .clone()
            .unwrap_or_else(|| format!("{}@{}", uuid::Uuid::new_v4(), domain(&from.addr)));
        let date = SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(u64::try_from(self.date).unwrap_or(0));
        let mut b = Message::builder()
            .from(mailbox(from)?)
            .date(date)
            .message_id(Some(format!("<{message_id}>")))
            .subject(self.subject.clone());
        for m in &self.to {
            b = b.to(mailbox(m)?);
        }
        for m in &self.cc {
            b = b.cc(mailbox(m)?);
        }
        if let Some(r) = self.reply_to.as_deref().filter(|r| !r.trim().is_empty()) {
            b = b.reply_to(mailbox(&MailboxSpec {
                name: String::new(),
                addr: r.to_string(),
            })?);
        }
        if let Some(id) = &self.in_reply_to {
            b = b.in_reply_to(format!("<{id}>"));
        }
        if !self.references.is_empty() {
            let refs: Vec<String> = self.references.iter().map(|r| format!("<{r}>")).collect();
            b = b.references(refs.join(" "));
        }
        // The envelope: every To, Cc and Bcc address; the Bcc header is never written.
        let mut to: Vec<String> = Vec::new();
        for m in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            let a = m.addr.trim().to_string();
            if !to.iter().any(|t| t.eq_ignore_ascii_case(&a)) {
                to.push(a);
            }
        }
        if to.is_empty() {
            return Err(SmtpError::Rejected("no recipients".into()));
        }
        let envelope = lettre::address::Envelope::new(
            Some(
                from.addr
                    .trim()
                    .parse()
                    .map_err(|_| SmtpError::Rejected("invalid sender".into()))?,
            ),
            to.iter()
                .map(|a| a.parse())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SmtpError::Rejected("invalid recipient".into()))?,
        )
        .map_err(|_| SmtpError::Rejected("invalid envelope".into()))?;
        b = b.envelope(envelope);

        let body: MultiPart = match &self.html {
            Some(html) => MultiPart::alternative()
                .singlepart(text_part(ContentType::TEXT_PLAIN, &self.text)?)
                .singlepart(text_part(ContentType::TEXT_HTML, html)?),
            None => MultiPart::mixed().singlepart(text_part(ContentType::TEXT_PLAIN, &self.text)?),
        };
        let has_files = !self.attachments.is_empty() || self.forwarded.is_some();
        let message = if has_files {
            let mut mixed = match &self.html {
                Some(_) => MultiPart::mixed().multipart(body),
                None => {
                    MultiPart::mixed().singlepart(text_part(ContentType::TEXT_PLAIN, &self.text)?)
                }
            };
            for a in &self.attachments {
                let content_type = ContentType::parse(&a.mime).unwrap_or(
                    ContentType::parse("application/octet-stream")
                        .map_err(|_| SmtpError::Rejected("content type".into()))?,
                );
                mixed = mixed.singlepart(
                    Attachment::new(a.filename.clone()).body(
                        Body::new_with_encoding(a.bytes.clone(), ContentTransferEncoding::Base64)
                            .map_err(|_| SmtpError::Rejected("attachment".into()))?,
                        content_type,
                    ),
                );
            }
            if let Some(raw) = &self.forwarded {
                mixed = mixed.singlepart(forwarded_part(raw)?);
            }
            b.multipart(mixed)
        } else {
            match &self.html {
                Some(_) => b.multipart(body),
                None => b.singlepart(text_part(ContentType::TEXT_PLAIN, &self.text)?),
            }
        }
        .map_err(|e| SmtpError::Rejected(format!("message: {e}")))?;
        Ok(BuiltMessage {
            raw: message.formatted(),
            message_id,
            envelope: crate::Envelope {
                from: from.addr.trim().to_string(),
                to,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, addr: &str) -> MailboxSpec {
        MailboxSpec {
            name: name.into(),
            addr: addr.into(),
        }
    }

    fn message() -> OutgoingMessage {
        OutgoingMessage {
            from: Some(spec("Jürgen Ölmann", "jurgen@example.test")),
            to: vec![spec("Bob", "bob@example.test")],
            cc: vec![spec("", "carol@example.test")],
            bcc: vec![
                spec("Secret", "hidden@example.test"),
                spec("", "BOB@example.test"),
            ],
            reply_to: Some("replies@example.test".into()),
            subject: "Grüße, Bob".into(),
            text: "Hello Bob, ça va? A long line ".repeat(5),
            html: Some("<p>Hello <b>Bob</b>, ça va?</p>".into()),
            in_reply_to: Some("parent@x.test".into()),
            references: vec!["root@x.test".into(), "parent@x.test".into()],
            date: 1_792_411_200,
            ..OutgoingMessage::default()
        }
    }

    #[test]
    fn headers_follow_section_7() {
        let built = message().build().unwrap();
        let raw = String::from_utf8(built.raw.clone()).unwrap();
        let head = raw.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
        assert!(head.contains(&format!(
            "message-id: <{}>",
            built.message_id.to_ascii_lowercase()
        )));
        assert!(built.message_id.ends_with("@example.test"));
        assert!(head.contains("date: mon, 19 oct 2026 12:00:00"));
        assert!(head.contains("from: =?utf-8?"), "RFC 2047 name: {head}");
        assert!(head.contains("subject: =?utf-8?"));
        assert!(head.contains("to: bob <bob@example.test>"));
        assert!(head.contains("cc: carol@example.test"));
        assert!(head.contains("reply-to: replies@example.test"));
        assert!(head.contains("in-reply-to: <parent@x.test>"));
        assert!(head.contains("references: <root@x.test> <parent@x.test>"));
        assert!(head.contains("mime-version: 1.0"));
        assert!(head.contains("content-type: multipart/alternative"));
        assert!(
            !raw.to_ascii_lowercase().contains("bcc"),
            "never a Bcc header"
        );
        assert!(!raw.contains("hidden@example.test"));
        assert!(!head.contains("x-mailer") && !head.contains("user-agent"));
        assert!(raw.contains("Content-Transfer-Encoding: quoted-printable"));
        assert!(raw.is_ascii());
        // The envelope has everyone once, Bcc included.
        assert_eq!(
            built.envelope.to,
            vec![
                "bob@example.test",
                "carol@example.test",
                "hidden@example.test"
            ]
        );
        assert_eq!(built.envelope.from, "jurgen@example.test");
    }

    #[test]
    fn plain_only_attachments_and_forwards() {
        let mut m = message();
        m.html = None;
        let raw = String::from_utf8(m.build().unwrap().raw).unwrap();
        assert!(raw.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(!raw.contains("multipart/alternative") && !raw.contains("text/html"));

        let mut m = message();
        m.attachments = vec![OutgoingAttachment {
            filename: "résumé.pdf".into(),
            mime: "application/pdf".into(),
            bytes: vec![0, 1, 2, 255],
        }];
        m.forwarded = Some(b"Subject: old\r\n\r\nold body\r\n".to_vec());
        let raw = String::from_utf8(m.build().unwrap().raw).unwrap();
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("Content-Transfer-Encoding: base64"));
        assert!(raw.contains("message/rfc822"));
        assert!(raw.contains("Subject: old"));

        let mut m = message();
        m.message_id = Some("draft-1@example.test".into());
        assert_eq!(m.build().unwrap().message_id, "draft-1@example.test");
        let mut m = message();
        m.to.clear();
        m.cc.clear();
        m.bcc.clear();
        assert!(m.build().is_err(), "no recipients");
    }
}
