//! T19, Rust half (SPEC_GAPS G56): every case of the XSS corpus goes through the sanitizer
//! and URL rewriting, and nothing that could run, load or navigate survives. The WebView2
//! harness (tests/render/xss-harness.mjs) checks the same corpus in the real webview.

#![cfg(test)]

use mail_render::{body_document, csp, sanitize, BodyRequest, UrlForm};
use mail_store::Body;

const CORPUS: &str = include_str!("../../../tests/fixtures/xss/corpus.html");

fn cases() -> Vec<(String, String)> {
    CORPUS
        .split("<!-- case: ")
        .skip(1)
        .map(|chunk| {
            let (name, html) = chunk.split_once(" -->").unwrap();
            (name.trim().to_string(), html.trim().to_string())
        })
        .collect()
}

/// The markup with every `data-href="..."` value removed: a link target is reported to the
/// parent on click and never loaded by the frame.
fn without_link_targets(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(at) = rest.find(" data-href=\"") {
        out.push_str(&rest[..at]);
        let after = &rest[at + " data-href=\"".len()..];
        rest = after.find('"').map_or("", |end| &after[end + 1..]);
    }
    out.push_str(rest);
    out
}

/// The tags and the CSS of the markup, without text content: escaped text such as
/// `fetch('http://...')` is inert and stays readable.
fn markup_only(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut in_style = false;
    let mut quoted = false;
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    for c in html.chars() {
        let rest = &lower[i..];
        if !in_tag && c == '<' {
            in_tag = true;
            if rest.starts_with("</style") {
                in_style = false;
            } else if rest.starts_with("<style") {
                in_style = true;
            }
        }
        if in_tag || in_style {
            out.push(c);
        }
        if in_tag && c == '"' {
            quoted = !quoted;
        }
        if in_tag && !quoted && c == '>' {
            in_tag = false;
            out.push(' ');
        }
        i += c.len_utf8();
    }
    out
}

/// Every attribute name in the markup.
fn attribute_names(html: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_tag = false;
    let mut quote: Option<char> = None;
    let mut word = String::new();
    for c in html.chars() {
        match (in_tag, quote, c) {
            (false, _, '<') => in_tag = true,
            (true, None, '>') => in_tag = false,
            (true, None, '"' | '\'') => quote = Some(c),
            (true, Some(q), c) if c == q => quote = None,
            (true, None, '=') => {
                names.push(std::mem::take(&mut word).to_ascii_lowercase());
            }
            (true, None, c) if c.is_whitespace() => word.clear(),
            (true, None, c) => word.push(c),
            _ => {}
        }
    }
    names
}

const FORBIDDEN_TAGS: &[&str] = &[
    "<script",
    "<iframe",
    "<frame",
    "<object",
    "<embed",
    "<applet",
    "<svg",
    "<math",
    "<form",
    "<input",
    "<button",
    "<textarea",
    "<select",
    "<base",
    "<meta",
    "<link",
    "<video",
    "<audio",
    "<noscript",
    "<template",
    "<image",
    "<use",
    "<animate",
    "<marquee",
];

const FORBIDDEN_ATTRIBUTES: &[&str] = &[
    "srcdoc",
    "srcset",
    "poster",
    "action",
    "formaction",
    "ping",
    "target",
    "id",
    "name",
    "http-equiv",
    "xlink:href",
    "codebase",
    "data",
];

#[test]
fn t19_nothing_executable_or_fetchable_survives() {
    let cases = cases();
    assert!(cases.len() >= 40, "corpus has {} cases", cases.len());
    for form in [UrlForm::Custom, UrlForm::Http] {
        for (name, html) in &cases {
            let s = sanitize(html, 9, form);
            let lower = s.html.to_ascii_lowercase();
            for tag in FORBIDDEN_TAGS {
                assert!(!lower.contains(tag), "{name}: {tag} survived: {}", s.html);
            }
            for attr in attribute_names(&lower) {
                assert!(
                    !attr.starts_with("on"),
                    "{name}: event handler {attr}: {}",
                    s.html
                );
                assert!(
                    !FORBIDDEN_ATTRIBUTES.contains(&attr.as_str()),
                    "{name}: attribute {attr}: {}",
                    s.html
                );
            }
            let fetchable = without_link_targets(&markup_only(&lower));
            for needle in [
                "javascript:",
                "vbscript:",
                "http://beacon",
                "https://beacon",
                "//beacon",
                "@import",
                "expression(",
                "data:text",
            ] {
                assert!(
                    !fetchable.contains(needle),
                    "{name}: {needle} survived: {}",
                    s.html
                );
            }
            // Every remaining URL is one of ours.
            for raw in fetchable.split(['"', '\'', '(', ')', ' ', '&', ';']) {
                if let Some(scheme) = raw.split_once("://").map(|(s, _)| s) {
                    let ours = match form {
                        UrlForm::Custom => matches!(scheme, "mail-remote" | "mail-cid"),
                        UrlForm::Http => {
                            raw.starts_with("http://mail-remote.localhost/")
                                || raw.starts_with("http://mail-cid.localhost/")
                        }
                    };
                    assert!(ours, "{name}: foreign URL {raw}: {}", s.html);
                }
            }
            // The served document has exactly one script, ours, and the CSP keeps
            // mail-remote out until images are allowed.
            let body = Body {
                text_plain: String::new(),
                text_html: html.clone(),
                fetched_at: 0,
            };
            let doc = body_document(&body, 9, BodyRequest::default(), form, "n0nce");
            assert_eq!(
                doc.to_ascii_lowercase().matches("<script").count(),
                1,
                "{name}"
            );
            assert!(doc.contains("<script nonce=\"n0nce\">"), "{name}");
        }
    }
    assert!(!csp("n", false).contains("mail-remote"));
}

#[test]
fn t19_links_survive_only_as_reported_targets() {
    let (_, html) = cases()
        .into_iter()
        .find(|(n, _)| n == "target-blank")
        .unwrap();
    let s = sanitize(&html, 1, UrlForm::Custom);
    assert_eq!(
        s.html,
        "<a href=\"#\" data-href=\"http://beacon.invalid/link-click\">only on click</a>"
    );
}
