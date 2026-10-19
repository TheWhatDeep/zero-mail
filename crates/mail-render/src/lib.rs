#![forbid(unsafe_code)]
//! `render` (BUILD.md section 3): MIME to display model, sanitizer, and the `mail-body`,
//! `mail-cid` and `mail-remote` protocols. May call `store`; reaches the network only
//! through the remote-image proxy.
//!
//! The HTML pipeline (section 8), every step required, in order:
//! 1. **Sanitize** with an `ammonia` allowlist ([`sanitize()`]).
//! 2. **Rewrite URLs**: `cid:` to `mail-cid`, `http(s)` images and CSS `url()` to
//!    `mail-remote`, each `a href` to `data-href` with `href="#"`.
//! 3. **Serve** from `mail-body://<message>?images=0|1&plain=0|1` ([`mail_body`]) with the
//!    CSP as a response header and a fresh nonce for our one script (SPEC_GAPS G53).
//! 4. **Display** in `<iframe sandbox="allow-scripts">` without `allow-same-origin`, on white.
//!
//! Plain text is escaped, linkified and has deep quotes collapsed ([`plain`]).

mod css;
pub mod files;
mod plain;
pub mod remote;
mod sanitize;
pub mod urls;

pub use files::{display_name, is_executable, safe_name, AttachmentCache, CacheKey, TempFiles};
pub use remote::{RemoteError, RemoteProxy};
pub use sanitize::{sanitize, Sanitized, REMOVED_WITH_CONTENT};
pub use urls::UrlForm;

use mail_store::{Body, MessageId, Store, StoreError};

/// One custom protocol response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyResponse {
    pub status: u16,
    pub content_type: String,
    /// The `Content-Security-Policy` header value (section 8, step 3).
    pub csp: String,
    pub body: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What a `mail-body` request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BodyRequest {
    /// `images=1`: the CSP admits `mail-remote:`. Core sets it only for allowed messages.
    pub images: bool,
    /// `plain=1`: the plain-text part even when HTML exists (the per-message toggle).
    pub plain: bool,
}

impl BodyRequest {
    /// Reads `images=1` and `plain=1` from a query string.
    pub fn from_query(query: &str) -> Self {
        let has = |kv: &str| query.split('&').any(|p| p == kv);
        Self {
            images: has("images=1"),
            plain: has("plain=1"),
        }
    }
}

/// The frame's only script (it carries the nonce): it posts the content height for
/// auto-sizing, and reports link clicks and the hovered link target to the parent, which
/// decides what to do (section 8, Links). No navigation happens inside the frame.
const FRAME_SCRIPT: &str = "(function(){\
var last=0,hover=null;\
function post(m){parent.postMessage(m,'*');}\
function size(){var h=document.documentElement.scrollHeight;if(h!==last){last=h;post({zeromail:'height',height:h});}}\
addEventListener('load',size);new ResizeObserver(size).observe(document.documentElement);\
function link(t){while(t&&t.nodeType===1){if(t.hasAttribute('data-href'))return t;t=t.parentNode;}return null;}\
function anchor(t){while(t&&t.nodeType===1){if(t.tagName==='A')return t;t=t.parentNode;}return null;}\
addEventListener('click',function(e){var a=link(e.target);if(a||anchor(e.target)){e.preventDefault();}\
if(a){post({zeromail:'link',href:a.getAttribute('data-href'),text:(a.textContent||'').slice(0,2048)});}},true);\
addEventListener('mouseover',function(e){var a=link(e.target);var h=a?a.getAttribute('data-href'):null;\
if(h!==hover){hover=h;post({zeromail:'hover',href:h});}});\
document.addEventListener('mouseleave',function(){if(hover!==null){hover=null;post({zeromail:'hover',href:null});}});\
addEventListener('submit',function(e){e.preventDefault();},true);\
})();";

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

/// The white canvas both documents sit on (section 8, step 4; no color inversion).
const CANVAS: &str = "html,body{margin:0;background:#ffffff;color:#3f3f46;}\
body{font:13px/1.6 system-ui,-apple-system,\"Segoe UI\",Roboto,sans-serif;}";

fn document(style: &str, body: &str, nonce: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{CANVAS}{style}</style></head>\
         <body>{body}<script nonce=\"{nonce}\">{FRAME_SCRIPT}</script></body></html>"
    )
}

/// A plain-text body: escaped, linkified, deep quotes collapsed.
pub fn plain_document(text: &str, nonce: &str) -> String {
    document(
        "body{white-space:pre-wrap;overflow-wrap:anywhere;}\
         details.quote>summary{cursor:pointer;}",
        &plain::to_html(text),
        nonce,
    )
}

/// An HTML body after steps 1 and 2.
pub fn html_document(sanitized: &str, nonce: &str) -> String {
    document("body{overflow-wrap:break-word;}", sanitized, nonce)
}

/// The document for a cached body: HTML when there is HTML and plain text was not asked
/// for, else the plain text.
pub fn body_document(
    body: &Body,
    message_id: MessageId,
    request: BodyRequest,
    form: UrlForm,
    nonce: &str,
) -> String {
    if !request.plain && !body.text_html.trim().is_empty() {
        html_document(&sanitize(&body.text_html, message_id, form).html, nonce)
    } else {
        plain_document(&body.text_plain, nonce)
    }
}

/// How many remote images a body's HTML refers to (section 8, Remote images).
pub fn remote_image_count(body: &Body, form: UrlForm) -> usize {
    if body.text_html.trim().is_empty() {
        0
    } else {
        sanitize(&body.text_html, 0, form).remote_images
    }
}

/// The `mail-body` response. A message without a cached body is a 404; the reader shows its
/// loading state and asks again after the body arrives.
pub async fn mail_body(
    store: &Store,
    message_id: MessageId,
    request: BodyRequest,
    form: UrlForm,
) -> Result<BodyResponse, RenderError> {
    let n = nonce();
    let body = match store.get_body(message_id).await {
        Ok(body) => body,
        Err(StoreError::NotFound) => None,
        Err(e) => return Err(e.into()),
    };
    let (status, html) = match body {
        Some(b) => (200, body_document(&b, message_id, request, form, &n)),
        None => (404, plain_document("", &n)),
    };
    Ok(BodyResponse {
        status,
        content_type: "text/html; charset=utf-8".into(),
        csp: csp(&n, request.images),
        body: html.into_bytes(),
    })
}

/// An image response for `mail-cid` and `mail-remote`. Only `image/*` is ever served.
pub fn image_response(content_type: &str, bytes: Vec<u8>) -> BodyResponse {
    let ok = content_type.to_ascii_lowercase().starts_with("image/");
    BodyResponse {
        status: if ok { 200 } else { 404 },
        content_type: if ok {
            content_type.to_string()
        } else {
            "text/plain".into()
        },
        csp: "default-src 'none'".into(),
        body: if ok { bytes } else { Vec::new() },
    }
}

/// An empty response with `status`.
pub fn empty_response(status: u16) -> BodyResponse {
    BodyResponse {
        status,
        content_type: "text/plain".into(),
        csp: "default-src 'none'".into(),
        body: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_escaped_and_only_our_script_runs() {
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
    fn html_or_plain_by_request() {
        let body = Body {
            text_plain: "plain words".into(),
            text_html: "<p>html <b>words</b></p><script>x()</script>".into(),
            fetched_at: 0,
        };
        let html = body_document(&body, 1, BodyRequest::default(), UrlForm::Custom, "n");
        assert!(html.contains("<p>html <b>words</b></p>") && !html.contains("x()"));
        let plain = body_document(
            &body,
            1,
            BodyRequest {
                plain: true,
                images: false,
            },
            UrlForm::Custom,
            "n",
        );
        assert!(plain.contains("plain words") && !plain.contains("<b>"));
        assert_eq!(
            BodyRequest::from_query("images=1&plain=1"),
            BodyRequest {
                images: true,
                plain: true
            }
        );
        assert_eq!(BodyRequest::from_query("images=0"), BodyRequest::default());
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

    #[test]
    fn only_images_are_served_as_images() {
        assert_eq!(image_response("image/png", b"x".to_vec()).status, 200);
        let html = image_response("text/html", b"<script>".to_vec());
        assert_eq!((html.status, html.body.len()), (404, 0));
    }
}
