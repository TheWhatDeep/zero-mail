//! The IMAP debug trace (section 13, item 8): every line on the wire at `debug` level under
//! the target `mail_imap::trace`, off unless the debug switch is on. Nothing secret or
//! personal is written: `LOGIN` and `AUTHENTICATE` arguments, SASL responses and server
//! challenges are replaced, every literal (message bodies, header blocks, APPEND data) is
//! reduced to its length, and quoted strings in `FETCH` responses are blanked.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The `tracing` target of the trace.
pub const TARGET: &str = "mail_imap::trace";

/// Longest line kept; the rest of a longer line is dropped from the trace.
const MAX_LINE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Client,
    Server,
}

/// Splits one direction of the byte stream into lines and literals.
#[derive(Debug)]
struct Framer {
    direction: Direction,
    line: Vec<u8>,
    /// Bytes of a literal still to skip.
    literal: u64,
}

impl Framer {
    fn new(direction: Direction) -> Self {
        Self {
            direction,
            line: Vec::new(),
            literal: 0,
        }
    }

    fn feed(&mut self, mut bytes: &[u8], out: &mut Vec<String>) {
        while !bytes.is_empty() {
            if self.literal > 0 {
                let n = usize::try_from(self.literal)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                self.literal -= n as u64;
                bytes = &bytes[n..];
                continue;
            }
            match bytes.iter().position(|b| *b == b'\n') {
                Some(end) => {
                    if self.line.len() < MAX_LINE {
                        self.line.extend_from_slice(&bytes[..end]);
                    }
                    bytes = &bytes[end + 1..];
                    let line = String::from_utf8_lossy(&self.line)
                        .trim_end_matches('\r')
                        .to_string();
                    self.line.clear();
                    if let Some(n) = literal_length(&line) {
                        self.literal = n;
                    }
                    out.push(match self.direction {
                        Direction::Client => redact_client(&line),
                        Direction::Server => redact_server(&line),
                    });
                }
                None => {
                    let room = MAX_LINE.saturating_sub(self.line.len());
                    self.line.extend_from_slice(&bytes[..bytes.len().min(room)]);
                    bytes = &[];
                }
            }
        }
    }
}

/// `{n}` or `{n+}` at the end of a line announces an n-byte literal.
fn literal_length(line: &str) -> Option<u64> {
    let inner = line.strip_suffix('}')?;
    let open = inner.rfind('{')?;
    inner[open + 1..].trim_end_matches('+').parse().ok()
}

/// Replaces a trailing literal header with its length only.
fn mark_literal(line: &str) -> String {
    match literal_length(line) {
        Some(n) => {
            let open = line.rfind('{').unwrap_or(line.len());
            format!("{}[literal {n} bytes]", &line[..open])
        }
        None => line.to_string(),
    }
}

pub(crate) fn redact_client(line: &str) -> String {
    let mut words = line.split_whitespace();
    let (Some(tag), Some(command)) = (words.next(), words.next()) else {
        // A lone token after AUTHENTICATE is a SASL response; DONE ends IDLE.
        return if line.trim() == "DONE" || line.trim().is_empty() {
            line.to_string()
        } else {
            "[redacted]".to_string()
        };
    };
    let upper = command.to_ascii_uppercase();
    if upper == "LOGIN" || upper == "AUTHENTICATE" {
        return format!("{tag} {command} [redacted]");
    }
    mark_literal(line)
}

pub(crate) fn redact_server(line: &str) -> String {
    if line.starts_with('+') {
        return "+ [redacted]".to_string();
    }
    let line = mark_literal(line);
    if !line.to_ascii_uppercase().contains(" FETCH ") {
        return line;
    }
    // Header values and other message data never reach the log.
    let mut out = String::with_capacity(line.len());
    let mut quoted = false;
    let mut escaped = false;
    for c in line.chars() {
        if quoted {
            match (escaped, c) {
                (false, '\\') => escaped = true,
                (false, '"') => {
                    quoted = false;
                    out.push('"');
                }
                _ => escaped = false,
            }
        } else {
            if c == '"' {
                quoted = true;
                out.push_str("\"…");
                continue;
            }
            out.push(c);
        }
    }
    out
}

/// A stream that writes the trace while passing bytes through unchanged.
#[derive(Debug)]
pub(crate) struct Traced<S> {
    inner: S,
    /// `None` when the trace was off at connect time: no buffering at all.
    framers: Option<(Framer, Framer)>,
}

impl<S> Traced<S> {
    pub(crate) fn new(inner: S) -> Self {
        let on = tracing::enabled!(target: TARGET, tracing::Level::DEBUG);
        Self {
            inner,
            framers: on.then(|| {
                (
                    Framer::new(Direction::Client),
                    Framer::new(Direction::Server),
                )
            }),
        }
    }

    fn log(&mut self, direction: Direction, bytes: &[u8]) {
        let Some((client, server)) = self.framers.as_mut() else {
            return;
        };
        let mut lines = Vec::new();
        match direction {
            Direction::Client => client.feed(bytes, &mut lines),
            Direction::Server => server.feed(bytes, &mut lines),
        }
        for line in lines {
            match direction {
                Direction::Client => tracing::debug!(target: TARGET, "C: {line}"),
                Direction::Server => tracing::debug!(target: TARGET, "S: {line}"),
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Traced<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let poll = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = poll {
            let new = buf.filled()[before..].to_vec();
            this.log(Direction::Server, &new);
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Traced<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = poll {
            this.log(Direction::Client, &buf[..n]);
        }
        poll
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(direction: Direction, chunks: &[&[u8]]) -> Vec<String> {
        let mut f = Framer::new(direction);
        let mut out = Vec::new();
        for c in chunks {
            f.feed(c, &mut out);
        }
        out
    }

    #[test]
    fn credentials_and_sasl_are_redacted() {
        let lines = frame(
            Direction::Client,
            &[
                b"a1 LOGIN alice \"hunter2\"\r\n",
                b"a2 AUTHENTICATE PLAIN\r\n",
                b"AGFsaWNlAGh1bnRlcjI=\r\n",
                b"a3 SELECT INBOX\r\nDONE\r\n",
            ],
        );
        assert_eq!(
            lines,
            vec![
                "a1 LOGIN [redacted]",
                "a2 AUTHENTICATE [redacted]",
                "[redacted]",
                "a3 SELECT INBOX",
                "DONE"
            ]
        );
        assert!(!lines.join("\n").contains("hunter2"));
    }

    #[test]
    fn literals_and_fetched_text_never_appear() {
        let lines = frame(
            Direction::Server,
            &[
                b"* 1 FETCH (UID 7 FLAGS (\\Seen) BODY[HEADER.FIELDS (SUBJECT)] {24}\r\n",
                b"Subject: Secret pl",
                b"an\r\n\r\n)\r\n",
                b"* 2 FETCH (UID 8 X-GM-LABELS (\"Private \\\"label\\\"\"))\r\n",
                b"+ eyJzdGF0dXMiOiI0MDEifQ==\r\n",
                b"a4 OK done\r\n",
            ],
        );
        let all = lines.join("\n");
        assert!(!all.contains("Secret") && !all.contains("Private"), "{all}");
        assert_eq!(
            lines[0],
            "* 1 FETCH (UID 7 FLAGS (\\Seen) BODY[HEADER.FIELDS (SUBJECT)] [literal 24 bytes]"
        );
        assert_eq!(lines[1], ")");
        assert_eq!(lines[2], "* 2 FETCH (UID 8 X-GM-LABELS (\"…\"))");
        assert_eq!(lines[3], "+ [redacted]");
        assert_eq!(lines[4], "a4 OK done");

        let append = frame(
            Direction::Client,
            &[b"a5 APPEND Sent (\\Seen) {11}\r\nHello Bob\r\n", b"\r\n"],
        );
        assert_eq!(
            append,
            vec!["a5 APPEND Sent (\\Seen) [literal 11 bytes]", ""]
        );
    }
}
