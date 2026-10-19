//! The SMTP debug trace (section 13, item 8): every line on the wire at `debug` level under
//! the target `mail_smtp::trace`, off unless the debug switch is on. `AUTH` arguments and
//! SASL responses are replaced, server challenges are blanked, and the message between
//! `DATA` and the final `.` is reduced to its length.

/// The `tracing` target of the trace.
pub const TARGET: &str = "mail_smtp::trace";

/// Longest line kept; the rest of a longer line is dropped from the trace.
const MAX_LINE: usize = 4096;

/// One-word client commands that are logged as they are.
const BARE_COMMANDS: &[&str] = &["DATA", "QUIT", "RSET", "NOOP", "STARTTLS"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Client,
    Server,
}

/// Splits one direction of the stream into lines; the client side also tracks DATA.
#[derive(Debug)]
pub(crate) struct Framer {
    direction: Direction,
    line: Vec<u8>,
    /// Inside the message after `DATA`: bytes seen so far.
    data: Option<u64>,
}

impl Framer {
    pub(crate) fn new(direction: Direction) -> Self {
        Self {
            direction,
            line: Vec::new(),
            data: None,
        }
    }

    pub(crate) fn feed(&mut self, mut bytes: &[u8], out: &mut Vec<String>) {
        while !bytes.is_empty() {
            let Some(end) = bytes.iter().position(|b| *b == b'\n') else {
                let room = MAX_LINE.saturating_sub(self.line.len());
                if let Some(n) = self.data.as_mut() {
                    *n += bytes.len() as u64;
                }
                self.line.extend_from_slice(&bytes[..bytes.len().min(room)]);
                return;
            };
            if let Some(n) = self.data.as_mut() {
                *n += end as u64 + 1;
            }
            if self.line.len() < MAX_LINE {
                self.line.extend_from_slice(&bytes[..end]);
            }
            bytes = &bytes[end + 1..];
            let line = String::from_utf8_lossy(&self.line)
                .trim_end_matches('\r')
                .to_string();
            self.line.clear();
            if let Some(n) = self.data {
                if line == "." {
                    self.data = None;
                    out.push(format!("[message {} bytes]", n.saturating_sub(3)));
                    out.push(".".into());
                }
                continue;
            }
            out.push(match self.direction {
                Direction::Client => self.client_line(&line),
                Direction::Server => server_line(&line),
            });
        }
    }

    fn client_line(&mut self, line: &str) -> String {
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or_default().to_ascii_uppercase();
        if first == "AUTH" {
            return format!("AUTH {} [redacted]", words.next().unwrap_or_default());
        }
        if first == "DATA" {
            self.data = Some(0);
            return line.to_string();
        }
        if words.next().is_none() && !BARE_COMMANDS.contains(&first.as_str()) && !first.is_empty() {
            // A lone token that is no command is a SASL response.
            return "[redacted]".to_string();
        }
        line.to_string()
    }
}

/// `334` lines carry SASL challenges, blanked; everything else is protocol text.
fn server_line(line: &str) -> String {
    if line.starts_with("334") {
        "334 [redacted]".to_string()
    } else {
        line.to_string()
    }
}

/// Both directions of one connection, created only when the trace is on.
#[derive(Debug)]
pub(crate) struct Trace {
    client: Framer,
    server: Framer,
}

impl Trace {
    pub(crate) fn new_if_enabled() -> Option<Self> {
        tracing::enabled!(target: TARGET, tracing::Level::DEBUG).then(|| Self {
            client: Framer::new(Direction::Client),
            server: Framer::new(Direction::Server),
        })
    }

    pub(crate) fn log(&mut self, direction: Direction, bytes: &[u8]) {
        let mut lines = Vec::new();
        match direction {
            Direction::Client => self.client.feed(bytes, &mut lines),
            Direction::Server => self.server.feed(bytes, &mut lines),
        }
        for line in lines {
            match direction {
                Direction::Client => tracing::debug!(target: TARGET, "C: {line}"),
                Direction::Server => tracing::debug!(target: TARGET, "S: {line}"),
            }
        }
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
    fn auth_and_message_are_redacted() {
        let lines = frame(
            Direction::Client,
            &[
                b"EHLO [127.0.0.1]\r\nAUTH PLAIN AGFsaWNlAGh1bnRlcjI=\r\n",
                b"AUTH LOGIN\r\nYWxpY2U=\r\naHVudGVyMg==\r\n",
                b"MAIL FROM:<a@x.test>\r\nRCPT TO:<b@x.test>\r\nDATA\r\n",
                b"Subject: Secret\r\n\r\nThe plan",
                b"\r\n.\r\nQUIT\r\n",
            ],
        );
        let all = lines.join("\n");
        assert!(
            !all.contains("AGFsaWNl") && !all.contains("YWxpY2U") && !all.contains("Secret"),
            "{all}"
        );
        assert_eq!(
            lines,
            vec![
                "EHLO [127.0.0.1]",
                "AUTH PLAIN [redacted]",
                "AUTH LOGIN [redacted]",
                "[redacted]",
                "[redacted]",
                "MAIL FROM:<a@x.test>",
                "RCPT TO:<b@x.test>",
                "DATA",
                "[message 29 bytes]",
                ".",
                "QUIT"
            ]
        );
        let server = frame(Direction::Server, &[b"334 VXNlcm5hbWU6\r\n250 OK\r\n"]);
        assert_eq!(server, vec!["334 [redacted]", "250 OK"]);
    }
}
