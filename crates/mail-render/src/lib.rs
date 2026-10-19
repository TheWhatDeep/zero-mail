#![forbid(unsafe_code)]
//! `render` (BUILD.md section 3): MIME to display model, sanitizer, and the `mail-body`,
//! `mail-cid` and `mail-remote` protocols. May call `store`.
//!
//! M4 builds the `mail-body` response for plain text (SPEC_GAPS G42). The cached text is
//! HTML-escaped and nothing else is added. It is served with the section 8 CSP as a
//! response header with a fresh nonce, and shown in `<iframe sandbox="allow-scripts">`
//! without `allow-same-origin`. The only script is ours, carrying the nonce: it posts the
//! content height to the parent for auto-sizing. M5 adds the HTML pipeline (ammonia, URL
//! rewriting, linkify) to this same protocol.

use mail_store::{MessageId, Store, StoreError};

/// One `mail-body` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyResponse {
    pub status: u16,
    pub content_type: &'static str,
    /// The `Content-Security-Policy` header value (section 8, step 3).
    pub csp: String,
    pub body: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The height-reporting script. It sends only a number.
const HEIGHT_SCRIPT: &str =
    "(function(){var last=0;function post(){var h=document.documentElement.scrollHeight;\
if(h!==last){last=h;parent.postMessage({zeromail:'height',height:h},'*');}}\
addEventListener('load',post);new ResizeObserver(post).observe(document.documentElement);})();";

/// A random nonce for one response (section 8: `nonce-<random per response>`).
pub fn nonce() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The section 8 CSP. `mail-remote:` is allowed only when images are on. Each scheme is also
/// listed in the `http://<scheme>.localhost` form that Windows webviews use for custom
/// protocols.
pub fn csp(nonce: &str, images: bool) -> String {
    let mut img = String::from("img-src mail-cid: http://mail-cid.localhost");
    if images {
        img.push_str(" mail-remote: http://mail-remote.localhost");
    }
    format!(
        "default-src 'none'; style-src 'unsafe-inline'; {img}; script-src 'nonce-{nonce}'; \
         base-uri 'none'; form-action 'none'"
    )
}

/// Escapes text for HTML element content and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A plain-text body as a document on the white message canvas (section 8, step 4).
pub fn plain_document(text: &str, nonce: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
         html,body{{margin:0;background:#ffffff;color:#3f3f46;}}\
         body{{font:13px/1.6 system-ui,-apple-system,\"Segoe UI\",Roboto,sans-serif;\
         white-space:pre-wrap;overflow-wrap:anywhere;}}</style></head>\
         <body>{}<script nonce=\"{nonce}\">{HEIGHT_SCRIPT}</script></body></html>",
        escape(text)
    )
}

/// The `mail-body://<message>?images=0|1` response. A message without a cached body is a
/// 404; the reader shows its loading state and asks again after the body arrives.
pub async fn mail_body(
    store: &Store,
    message_id: MessageId,
    images: bool,
) -> Result<BodyResponse, RenderError> {
    let n = nonce();
    let body = match store.get_body(message_id).await {
        Ok(body) => body,
        Err(StoreError::NotFound) => None,
        Err(e) => return Err(e.into()),
    };
    let (status, html) = match body {
        Some(b) => (200, plain_document(&b.text_plain, &n)),
        None => (404, plain_document("", &n)),
    };
    Ok(BodyResponse {
        status,
        content_type: "text/html; charset=utf-8",
        csp: csp(&n, images),
        body: html.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_escaped_and_nothing_else_is_added() {
        let hostile = "<script>alert(1)</script><img src=x onerror=alert(2)> & \"'";
        let doc = plain_document(hostile, "abc");
        assert!(doc.contains("&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src=x onerror=alert(2)&gt; &amp; &quot;&#39;"));
        // Exactly one script: ours, with the nonce.
        assert_eq!(doc.matches("<script").count(), 1);
        assert!(doc.contains("<script nonce=\"abc\">"));
        assert!(!doc.contains("<img"));
        assert!(
            doc.contains("background:#ffffff"),
            "always on the white canvas"
        );
    }

    #[test]
    fn csp_follows_section_8() {
        let off = csp("n1", false);
        assert!(off.starts_with("default-src 'none'; style-src 'unsafe-inline'; img-src mail-cid:"));
        assert!(off.contains("script-src 'nonce-n1'"));
        assert!(!off.contains("mail-remote"));
        assert!(!off.contains("unsafe-eval"));
        let on = csp("n2", true);
        assert!(on.contains("mail-remote:"));
    }

    #[test]
    fn nonces_are_fresh() {
        let a = nonce();
        let b = nonce();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
