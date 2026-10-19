//! The built-in server table (section 5, step 2): Google, Fastmail, iCloud and Yahoo.

use crate::{Endpoint, Security, ServerSettings, Source};

struct Provider {
    domains: &'static [&'static str],
    imap: (&'static str, u16, Security),
    smtp: (&'static str, u16, Security),
}

const PROVIDERS: &[Provider] = &[
    Provider {
        domains: &["gmail.com", "googlemail.com"],
        imap: ("imap.gmail.com", 993, Security::Tls),
        smtp: ("smtp.gmail.com", 465, Security::Tls),
    },
    Provider {
        domains: &["fastmail.com", "fastmail.fm"],
        imap: ("imap.fastmail.com", 993, Security::Tls),
        smtp: ("smtp.fastmail.com", 465, Security::Tls),
    },
    Provider {
        domains: &["icloud.com", "me.com", "mac.com"],
        imap: ("imap.mail.me.com", 993, Security::Tls),
        smtp: ("smtp.mail.me.com", 587, Security::StartTls),
    },
    Provider {
        domains: &["yahoo.com", "ymail.com", "rocketmail.com"],
        imap: ("imap.mail.yahoo.com", 993, Security::Tls),
        smtp: ("smtp.mail.yahoo.com", 465, Security::Tls),
    },
];

/// The built-in settings for a domain. Each of these providers takes the full address as
/// the username.
pub(crate) fn lookup(domain: &str, email: &str) -> Option<ServerSettings> {
    let p = PROVIDERS.iter().find(|p| p.domains.contains(&domain))?;
    let endpoint = |(host, port, security): (&str, u16, Security)| Endpoint {
        host: host.to_string(),
        port,
        security,
    };
    Some(ServerSettings {
        imap: endpoint(p.imap),
        smtp: endpoint(p.smtp),
        username: email.trim().to_string(),
        source: Source::BuiltIn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_domains() {
        let g = lookup("gmail.com", "me@gmail.com").unwrap();
        assert_eq!(g.imap.host, "imap.gmail.com");
        assert_eq!((g.smtp.port, g.smtp.security), (465, Security::Tls));
        assert_eq!(g.username, "me@gmail.com");
        let i = lookup("me.com", "me@me.com").unwrap();
        assert_eq!((i.smtp.port, i.smtp.security), (587, Security::StartTls));
        assert!(lookup("example.com", "me@example.com").is_none());
    }
}
