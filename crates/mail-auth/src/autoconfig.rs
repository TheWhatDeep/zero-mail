//! Server-settings resolution (section 5, step 2; SPEC_GAPS G26, G27, G29).
//!
//! Order: the built-in table, the domain's own autoconfig file (two standard URLs), then the
//! Mozilla ISP database. The first source with a usable answer wins; with none, the UI shows
//! the manual form. Lookups use HTTPS only and send only the domain, never the address, and
//! the caller turns them off with the `online` flag (section 13, item 1).

use std::future::Future;

use crate::xml::{self, Element};
use crate::{builtin, split_address, AuthError, Endpoint, Security, ServerSettings, Source};

/// Largest autoconfig document read.
pub const AUTOCONFIG_MAX_BYTES: usize = 256 * 1024;

/// Most candidates returned for one address.
const MAX_CANDIDATES: usize = 3;

/// Fetches one HTTPS document. `None` on any failure (unreachable, not found, too large,
/// bad certificate): a failed lookup only moves resolution to the next source.
pub trait HttpGet: Send + Sync {
    fn get(&self, url: &str) -> impl Future<Output = Option<Vec<u8>>> + Send;
}

/// Server-settings candidates for an address, best first. Empty means: show the manual form.
pub async fn resolve(
    email: &str,
    online: bool,
    http: &impl HttpGet,
) -> Result<Vec<ServerSettings>, AuthError> {
    let (local, domain) = split_address(email)?;
    let email = email.trim();
    if let Some(settings) = builtin::lookup(&domain, email) {
        return Ok(vec![settings]);
    }
    if !online {
        return Ok(Vec::new());
    }
    let sources = [
        (
            format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
            Source::Autoconfig,
        ),
        (
            format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
            Source::Autoconfig,
        ),
        (
            format!("https://autoconfig.thunderbird.net/v1.1/{domain}"),
            Source::IspDb,
        ),
    ];
    for (url, source) in sources {
        let Some(body) = http.get(&url).await else {
            continue;
        };
        let found = candidates(&body, local, &domain, email, source);
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

/// Reads an autoconfig document. Each usable IMAP server becomes one candidate, paired with
/// the first usable SMTP server. Plaintext endpoints are never offered (section 5).
fn candidates(
    body: &[u8],
    local: &str,
    domain: &str,
    email: &str,
    source: Source,
) -> Vec<ServerSettings> {
    if body.len() > AUTOCONFIG_MAX_BYTES {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(body) else {
        return Vec::new();
    };
    let Ok(root) = xml::parse(text) else {
        return Vec::new();
    };
    if root.name != "clientConfig" {
        return Vec::new();
    }
    let Some(provider) = root.children_named("emailProvider").next() else {
        return Vec::new();
    };
    let expand = |s: &str| {
        s.replace("%EMAILADDRESS%", email)
            .replace("%EMAILLOCALPART%", local)
            .replace("%EMAILDOMAIN%", domain)
    };
    let server = |el: &Element| -> Option<(Endpoint, String)> {
        let host = expand(el.child_text("hostname")?);
        let port: u16 = el.child_text("port")?.parse().ok()?;
        let security = match el.child_text("socketType")? {
            "SSL" => Security::Tls,
            "STARTTLS" => Security::StartTls,
            _ => return None,
        };
        let auths: Vec<&str> = el
            .children_named("authentication")
            .map(|a| a.text.trim())
            .collect();
        let password_ok = auths.is_empty()
            || auths
                .iter()
                .any(|a| matches!(*a, "password-cleartext" | "plain" | "OAuth2"));
        let valid_host = !host.is_empty()
            && host.len() <= 253
            && host
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '.');
        if !password_ok || !valid_host || port == 0 {
            return None;
        }
        let username = el
            .child_text("username")
            .map(expand)
            .unwrap_or_else(|| email.to_string());
        Some((
            Endpoint {
                host,
                port,
                security,
            },
            username,
        ))
    };
    let typed = |tag: &'static str, ty: &'static str| {
        provider
            .children_named(tag)
            .filter(move |el| el.attr("type") == Some(ty))
            .filter_map(server)
    };
    let Some((smtp, _)) = typed("outgoingServer", "smtp").next() else {
        return Vec::new();
    };
    typed("incomingServer", "imap")
        .take(MAX_CANDIDATES)
        .map(|(imap, username)| ServerSettings {
            imap,
            smtp: smtp.clone(),
            username,
            source,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        pages: BTreeMap<String, Vec<u8>>,
        asked: Mutex<Vec<String>>,
    }

    impl HttpGet for Fake {
        async fn get(&self, url: &str) -> Option<Vec<u8>> {
            self.asked.lock().unwrap().push(url.to_string());
            self.pages.get(url).cloned()
        }
    }

    const CONFIG: &str = r#"<?xml version="1.0"?>
<clientConfig version="1.1">
  <emailProvider id="example.com">
    <domain>example.com</domain>
    <incomingServer type="pop3">
      <hostname>pop.example.com</hostname><port>995</port><socketType>SSL</socketType>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>plain.%EMAILDOMAIN%</hostname><port>143</port><socketType>plain</socketType>
      <authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.%EMAILDOMAIN%</hostname><port>993</port><socketType>SSL</socketType>
      <authentication>password-cleartext</authentication>
      <username>%EMAILLOCALPART%</username>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap2.example.com</hostname><port>143</port><socketType>STARTTLS</socketType>
      <authentication>password-encrypted</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.com</hostname><port>587</port><socketType>STARTTLS</socketType>
      <authentication>password-cleartext</authentication>
      <username>%EMAILADDRESS%</username>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn builtin_wins_and_sends_nothing() {
        let http = Fake::default();
        let found = block_on(resolve("me@gmail.com", true, &http)).unwrap();
        assert_eq!(found[0].source, Source::BuiltIn);
        assert!(http.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn domain_autoconfig_then_ispdb_in_order_with_only_the_domain() {
        let http = Fake::default();
        assert!(block_on(resolve("Me@Example.com", true, &http))
            .unwrap()
            .is_empty());
        let asked = http.asked.lock().unwrap().clone();
        assert_eq!(
            asked,
            vec![
                "https://autoconfig.example.com/mail/config-v1.1.xml",
                "https://example.com/.well-known/autoconfig/mail/config-v1.1.xml",
                "https://autoconfig.thunderbird.net/v1.1/example.com",
            ]
        );
        assert!(asked
            .iter()
            .all(|u| !u.contains("Me@") && !u.contains("me@")));
        assert!(asked.iter().all(|u| u.starts_with("https://")));
    }

    #[test]
    fn reads_usable_servers_only() {
        let mut http = Fake::default();
        http.pages.insert(
            "https://autoconfig.thunderbird.net/v1.1/example.com".into(),
            CONFIG.as_bytes().to_vec(),
        );
        let found = block_on(resolve("me@example.com", true, &http)).unwrap();
        assert_eq!(found.len(), 1, "plain and CRAM-only servers are dropped");
        let c = &found[0];
        assert_eq!(c.source, Source::IspDb);
        assert_eq!(
            c.imap,
            Endpoint {
                host: "imap.example.com".into(),
                port: 993,
                security: Security::Tls
            }
        );
        assert_eq!(c.smtp.security, Security::StartTls);
        assert_eq!(c.username, "me");
    }

    #[test]
    fn offline_and_garbage() {
        let mut http = Fake::default();
        http.pages.insert(
            "https://autoconfig.example.com/mail/config-v1.1.xml".into(),
            b"<html>not it</html>".to_vec(),
        );
        assert!(block_on(resolve("me@example.com", false, &http))
            .unwrap()
            .is_empty());
        assert!(
            http.asked.lock().unwrap().is_empty(),
            "offline sends nothing"
        );
        assert!(block_on(resolve("me@example.com", true, &http))
            .unwrap()
            .is_empty());
        assert_eq!(
            block_on(resolve("not-an-address", true, &http)),
            Err(AuthError::InvalidAddress)
        );
        let huge = vec![b' '; AUTOCONFIG_MAX_BYTES + 1];
        assert!(candidates(&huge, "a", "b.com", "a@b.com", Source::IspDb).is_empty());
    }
}
