//! The HTML pipeline, steps 1 and 2 (section 8): sanitize with an `ammonia` allowlist, then
//! rewrite URLs. HTML is stored unsanitized and passes through here at every render
//! (invariant 4), so a fix here applies to old mail too.
//!
//! - Kept: structural, text, table, `img`, `a` and `style` elements, with presentational
//!   attributes, `style` and `class`.
//! - Removed with their content: `script`, `iframe`, `frame`, `object`, `embed`, `applet`,
//!   `svg`, `math`, `audio`, `video`, form controls and the other elements in
//!   [`REMOVED_WITH_CONTENT`]. Removed, content kept: `form` and any element not allowed.
//!   Void elements such as `base`, `meta` and `link` simply disappear.
//! - Every `on*` attribute, `id` and `name` are dropped (none is allowed), as are
//!   `javascript:`, `vbscript:`, relative and non-image `data:` URLs.
//! - `cid:` images become `mail-cid://<message>/<cid>`, every `http(s)` image, background and
//!   CSS `url()` becomes `mail-remote://<encoded url>`, and each `a href` moves to
//!   `data-href` with `href="#"`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::css;
use crate::urls::{self, UrlForm};

/// The sanitized body and what it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    pub html: String,
    /// How many references to remote images it holds (section 8, Remote images).
    pub remote_images: usize,
}

const TAGS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "article",
    "aside",
    "b",
    "bdi",
    "bdo",
    "big",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nav",
    "ol",
    "p",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
];

/// Elements removed together with everything inside them.
pub const REMOVED_WITH_CONTENT: &[&str] = &[
    "script", "noscript", "template", "iframe", "frame", "frameset", "object", "embed", "applet",
    "svg", "math", "audio", "video", "canvas", "map", "title", "textarea", "select", "button",
    "datalist", "option", "optgroup", "dialog",
];

const GENERIC_ATTRIBUTES: &[&str] = &[
    "style", "class", "dir", "lang", "title", "align", "valign", "width", "height", "bgcolor",
    "color", "border",
];

const TAG_ATTRIBUTES: &[(&str, &[&str])] = &[
    ("a", &["href"]),
    ("img", &["src", "alt", "hspace", "vspace"]),
    (
        "table",
        &["cellpadding", "cellspacing", "background", "summary"],
    ),
    ("td", &["colspan", "rowspan", "nowrap", "background"]),
    (
        "th",
        &["colspan", "rowspan", "nowrap", "background", "scope"],
    ),
    ("tr", &["background"]),
    ("font", &["face", "size"]),
    ("ol", &["start", "type", "reversed"]),
    ("ul", &["type"]),
    ("li", &["value"]),
    ("col", &["span"]),
    ("colgroup", &["span"]),
    ("details", &["open"]),
];

/// Schemes that survive ammonia's URL check; [`rewrite_image`] and the link rule narrow
/// them further per attribute.
const URL_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "cid", "data"];

/// Schemes a link may keep (the parent allows only these, section 8, Links).
const LINK_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

/// Marks a link target between ammonia and the final rewrite; never seen by the webview.
const LINK_MARK: &str = "zeromail-link:";

/// The rewrite rules for one message's references.
#[derive(Debug)]
struct Rewrite {
    message_id: i64,
    form: UrlForm,
    remote: AtomicUsize,
}

impl Rewrite {
    /// An image reference: `cid:` to `mail-cid`, `http(s)` to `mail-remote`, `data:image/`
    /// other than SVG kept (the CSP never loads it), anything else dropped.
    fn image(&self, url: &str) -> Option<String> {
        let url = url.trim();
        match urls::scheme(url).as_deref() {
            Some("http" | "https") => {
                self.remote.fetch_add(1, Ordering::Relaxed);
                Some(self.form.remote(url))
            }
            Some("cid") => {
                let cid = urls::decode(&url[4..]);
                let cid = cid.trim().trim_start_matches('<').trim_end_matches('>');
                Some(self.form.cid(self.message_id, cid))
            }
            // SVG is on the removed list in any form (section 8, step 1).
            Some("data") => {
                let mime = url[5..].trim_start().to_ascii_lowercase();
                (mime.starts_with("image/") && !mime.starts_with("image/svg"))
                    .then(|| url.to_string())
            }
            _ => None,
        }
    }
}

fn builder(rules: Arc<Rewrite>) -> ammonia::Builder<'static> {
    let mut b = ammonia::Builder::empty();
    b.tags(TAGS.iter().copied().collect())
        .clean_content_tags(REMOVED_WITH_CONTENT.iter().copied().collect())
        .generic_attributes(GENERIC_ATTRIBUTES.iter().copied().collect())
        .tag_attributes(
            TAG_ATTRIBUTES
                .iter()
                .map(|(tag, attrs)| (*tag, attrs.iter().copied().collect::<HashSet<_>>()))
                .collect::<HashMap<_, _>>(),
        )
        .url_schemes(URL_SCHEMES.iter().copied().collect())
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(move |element, attribute, value| {
            filter(&rules, element, attribute, value)
        });
    b
}

fn filter<'u>(
    rules: &Rewrite,
    element: &str,
    attribute: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    match (element, attribute) {
        ("img", "src") | (_, "background") => rules.image(value).map(Cow::Owned),
        ("a", "href") => {
            let scheme = urls::scheme(value)?;
            LINK_SCHEMES
                .contains(&scheme.as_str())
                .then(|| Cow::Owned(format!("{LINK_MARK}{}", urls::encode(value.trim()))))
        }
        (_, "style") => Some(Cow::Owned(css::rewrite(value, &|u| rules.image(u)))),
        _ => Some(Cow::Borrowed(value)),
    }
}

/// Runs steps 1 and 2 on one message's HTML.
pub fn sanitize(html: &str, message_id: i64, form: UrlForm) -> Sanitized {
    let rules = Arc::new(Rewrite {
        message_id,
        form,
        remote: AtomicUsize::new(0),
    });
    let cleaned = builder(Arc::clone(&rules)).clean(html).to_string();
    let html = rewrite_styles(&move_links(&cleaned), &rules);
    Sanitized {
        html,
        remote_images: rules.remote.load(Ordering::Relaxed),
    }
}

/// `href="zeromail-link:<encoded>"` becomes `href="#" data-href="<url>"`. The mark is only
/// ever produced by [`filter`]; if a message's own text happens to contain it, the result is
/// still escaped text, never markup.
fn move_links(html: &str) -> String {
    let needle = format!(" href=\"{LINK_MARK}");
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(found) = html[i..].find(&needle) {
        let start = i + found;
        out.push_str(&html[i..start]);
        let value_start = start + needle.len();
        let Some(len) = html[value_start..].find('"') else {
            break;
        };
        let target = urls::decode(&html[value_start..value_start + len]);
        out.push_str(" href=\"#\" data-href=\"");
        out.push_str(&crate::escape(&target));
        out.push('"');
        i = value_start + len + 1;
    }
    out.push_str(&html[i..]);
    out
}

/// Rewrites the CSS inside every kept `<style>` element. The parser ends a style element
/// at the first `</style`, so its serialized text contains none.
fn rewrite_styles(html: &str, rules: &Rewrite) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(found) = lower[i..].find("<style") {
        let tag = i + found;
        let Some(open_end) = lower[tag..].find('>').map(|n| tag + n + 1) else {
            break;
        };
        let Some(close) = lower[open_end..].find("</style").map(|n| open_end + n) else {
            break;
        };
        out.push_str(&html[i..open_end]);
        out.push_str(&css::rewrite(&html[open_end..close], &|u| rules.image(u)));
        i = close;
        out.push_str("</style");
        i += "</style".len();
    }
    out.push_str(&html[i..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(html: &str) -> Sanitized {
        sanitize(html, 42, UrlForm::Custom)
    }

    #[test]
    fn structure_and_style_are_kept() {
        let s = clean(
            "<html><head><title>T</title><style>p{color:red}</style></head>\
             <body><table width=\"600\" cellpadding=\"4\"><tr><td style=\"color:blue\">x</td></tr></table>\
             <p class=\"lead\" onclick=\"alert(1)\" id=\"p1\">Hi</p></body></html>",
        );
        assert!(s.html.contains("<style>p{color:red}</style>"), "{}", s.html);
        assert!(s.html.contains("<table width=\"600\" cellpadding=\"4\">"));
        assert!(s.html.contains("<td style=\"color:blue\">x</td>"));
        assert!(s.html.contains("<p class=\"lead\">Hi</p>"));
        assert!(
            !s.html.contains("onclick") && !s.html.contains("id=") && !s.html.contains("<title")
        );
        assert_eq!(s.remote_images, 0);
    }

    #[test]
    fn urls_are_rewritten() {
        let s = clean(
            "<img src=\"cid:logo@x\"><img src=\"https://t.test/pixel.gif?u=1&amp;v=2\">\
             <a href=\"https://bank.test/a?b=1&amp;c=2\">bank</a><a href=\"javascript:alert(1)\">j</a>\
             <a href=\"/relative\">r</a><table background=\"http://bg.test/b.png\"><tr><td>t</td></tr></table>\
             <div style=\"background:url('http://css.test/c.png')\">d</div>\
             <img src=\"data:image/png;base64,AAAA\"><img src=\"data:text/html,x\">",
        );
        let h = &s.html;
        assert!(
            h.contains("src=\"mail-cid://localhost/42/logo%40x\""),
            "{h}"
        );
        assert!(
            h.contains(
                "src=\"mail-remote://localhost/https%3A%2F%2Ft.test%2Fpixel.gif%3Fu%3D1%26v%3D2\""
            ),
            "{h}"
        );
        assert!(
            h.contains("<a href=\"#\" data-href=\"https://bank.test/a?b=1&amp;c=2\">bank</a>"),
            "{h}"
        );
        assert!(h.contains("<a>j</a>") && h.contains("<a>r</a>"), "{h}");
        assert!(
            h.contains("background=\"mail-remote://localhost/http%3A%2F%2Fbg.test%2Fb.png\""),
            "{h}"
        );
        assert!(
            h.contains("url(&quot;mail-remote://localhost/http%3A%2F%2Fcss.test%2Fc.png&quot;)"),
            "{h}"
        );
        assert!(h.contains("src=\"data:image/png;base64,AAAA\""));
        assert!(!h.contains("data:text"));
        assert_eq!(s.remote_images, 3);
    }

    #[test]
    fn style_elements_lose_imports_and_remote_urls() {
        let s = sanitize(
            "<style>@import 'http://evil.test/x.css'; body{background:url(http://evil.test/b.png)}</style>",
            1,
            UrlForm::Http,
        );
        assert!(
            !s.html.contains("@import") && !s.html.contains("x.css"),
            "{}",
            s.html
        );
        assert!(
            s.html
                .contains("url(\"http://mail-remote.localhost/http%3A%2F%2Fevil.test%2Fb.png\")"),
            "{}",
            s.html
        );
        assert!(!s.html.contains("url(http://evil"));
        assert_eq!(s.remote_images, 1);
    }

    #[test]
    fn link_mark_in_text_stays_text() {
        let s = clean("<p> href=\"zeromail-link:%3Cscript%3E\" </p>");
        assert!(!s.html.contains("<script"), "{}", s.html);
    }
}
