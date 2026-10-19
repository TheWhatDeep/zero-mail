//! The app's custom protocol URLs (section 8, step 2) and the percent-encoding they use.
//!
//! Windows and Android webviews reach a custom protocol as `http://<scheme>.localhost/...`;
//! elsewhere it is `<scheme>://localhost/...`. A document served from `mail-body` must use the
//! form of the platform it runs on, so every URL is written for one [`UrlForm`].

use std::fmt::Write as _;

/// How custom protocol URLs are written on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlForm {
    /// `mail-cid://localhost/...` (macOS, Linux).
    Custom,
    /// `http://mail-cid.localhost/...` (Windows).
    Http,
}

impl UrlForm {
    /// The form the current OS uses.
    pub fn native() -> Self {
        if cfg!(any(windows, target_os = "android")) {
            Self::Http
        } else {
            Self::Custom
        }
    }

    fn base(self, scheme: &str) -> String {
        match self {
            Self::Custom => format!("{scheme}://localhost/"),
            Self::Http => format!("http://{scheme}.localhost/"),
        }
    }

    /// `mail-cid://<message>/<cid>`: an inline image of one message.
    pub fn cid(self, message_id: i64, cid: &str) -> String {
        format!("{}{message_id}/{}", self.base("mail-cid"), encode(cid))
    }

    /// `mail-remote://<encoded url>`: a remote image through the proxy.
    pub fn remote(self, url: &str) -> String {
        format!("{}{}", self.base("mail-remote"), encode(url))
    }
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3 / 2);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Reverses [`encode`]. Malformed escapes are kept as they are; invalid UTF-8 is replaced.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if let (b'%', Some(h), Some(l)) = (
            bytes[i],
            bytes.get(i + 1).copied().and_then(hex),
            bytes.get(i + 2).copied().and_then(hex),
        ) {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The path of a custom protocol request in either form: everything after the host,
/// without the leading `/` and without the query.
pub fn request_path(uri_path: &str) -> &str {
    uri_path.trim_start_matches('/')
}

/// The scheme of a URL, lowercased, if it has one.
pub fn scheme(url: &str) -> Option<String> {
    let (scheme, _) = url.trim().split_once(':')?;
    let valid = !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_in_both_forms() {
        assert_eq!(
            UrlForm::Custom.cid(7, "a@b"),
            "mail-cid://localhost/7/a%40b"
        );
        assert_eq!(
            UrlForm::Http.cid(7, "a@b"),
            "http://mail-cid.localhost/7/a%40b"
        );
        assert_eq!(
            UrlForm::Http.remote("https://x.test/p?q=1"),
            "http://mail-remote.localhost/https%3A%2F%2Fx.test%2Fp%3Fq%3D1"
        );
        let url = "https://x.test/é?a=b&c=%41";
        assert_eq!(decode(&encode(url)), url);
        assert_eq!(decode("bad%zz%4"), "bad%zz%4");
        assert_eq!(
            scheme(" JavaScript:alert(1)").as_deref(),
            Some("javascript")
        );
        assert_eq!(scheme("no scheme here"), None);
    }
}
