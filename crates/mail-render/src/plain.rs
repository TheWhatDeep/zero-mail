//! Plain-text bodies (section 8, Plain text): HTML-escaped, `http(s)` and `mailto` links
//! made clickable, and quoted blocks deeper than level 1 collapsed behind a toggle.
//! `format=flowed` is undone when the part is decoded (sync), so the text here is already
//! joined.

use crate::escape;

/// Schemes linkified in plain text.
const LINK_PREFIXES: &[&str] = &["https://", "http://", "mailto:"];

/// The quote depth of a line: the number of leading `>`, spaces between them allowed.
fn depth(line: &str) -> usize {
    let mut n = 0;
    for c in line.chars() {
        match c {
            '>' => n += 1,
            ' ' if n > 0 => {}
            _ => break,
        }
    }
    n
}

/// The text as HTML for a `white-space: pre-wrap` body.
pub(crate) fn to_html(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut collapsed = false;
    for line in text.split('\n') {
        let deep = depth(line) > 1;
        if deep && !collapsed {
            out.push_str("<details class=\"quote\"><summary>\u{2026}</summary>");
            collapsed = true;
        } else if !deep && collapsed {
            // The last quoted line's newline is inside the block.
            out.push_str("</details>");
            collapsed = false;
        }
        out.push_str(&linkify(line));
        out.push('\n');
    }
    if collapsed {
        out.push_str("</details>");
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Escapes one line and turns `http(s)://` and `mailto:` URLs into links that the frame
/// script reports to the parent (section 8, Links).
pub(crate) fn linkify(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        let next = LINK_PREFIXES
            .iter()
            .filter_map(|p| lower[i..].find(p).map(|at| (i + at, p.len())))
            .filter(|(at, _)| {
                // A URL starts a word.
                *at == 0
                    || !line[..*at]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_alphanumeric)
            })
            .min_by_key(|(at, _)| *at);
        let Some((start, prefix)) = next else {
            break;
        };
        let end = url_end(line, start, prefix);
        if end == start + prefix {
            // A bare prefix with nothing after it.
            out.push_str(&escape(&line[i..end]));
            i = end;
            continue;
        }
        out.push_str(&escape(&line[i..start]));
        let url = &line[start..end];
        out.push_str("<a href=\"#\" data-href=\"");
        out.push_str(&escape(url));
        out.push_str("\">");
        out.push_str(&escape(url));
        out.push_str("</a>");
        i = end;
    }
    out.push_str(&escape(&line[i..]));
    out
}

/// Where a URL starting at `start` ends: at whitespace or `<>"`, without trailing
/// punctuation, and without a closing bracket that has no opening one in the URL.
fn url_end(line: &str, start: usize, prefix: usize) -> usize {
    let rest = &line[start..];
    let mut end = start
        + rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
            .unwrap_or(rest.len());
    loop {
        let Some(last) = line[start + prefix..end].chars().next_back() else {
            return end;
        };
        let url = &line[start..end];
        let unbalanced = |open: char, close: char| {
            last == close && url.matches(open).count() < url.matches(close).count()
        };
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'')
            || unbalanced('(', ')')
            || unbalanced('[', ']')
            || unbalanced('{', '}')
        {
            end -= last.len_utf8();
        } else {
            return end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_made_only_for_http_and_mailto() {
        assert_eq!(
            linkify("See https://x.test/a?b=1&c=2. Or (http://y.test/p_(q)) mailto:a@b.test!"),
            "See <a href=\"#\" data-href=\"https://x.test/a?b=1&amp;c=2\">https://x.test/a?b=1&amp;c=2</a>. \
             Or (<a href=\"#\" data-href=\"http://y.test/p_(q)\">http://y.test/p_(q)</a>) \
             <a href=\"#\" data-href=\"mailto:a@b.test\">mailto:a@b.test</a>!"
        );
        assert_eq!(
            linkify("ftp://x javascript:alert(1) file:///c"),
            "ftp://x javascript:alert(1) file:///c"
        );
        assert_eq!(linkify("<b>http://</b>"), "&lt;b&gt;http://&lt;/b&gt;");
        assert_eq!(linkify("xhttp://a.test"), "xhttp://a.test");
        assert_eq!(
            linkify("<https://a.test/>"),
            "&lt;<a href=\"#\" data-href=\"https://a.test/\">https://a.test/</a>&gt;"
        );
    }

    #[test]
    fn deep_quotes_collapse() {
        let html = to_html("Hi\r\n> one\r\n>> two\r\n> > three\r\n> back\r\nend");
        assert_eq!(
            html,
            "Hi\n&gt; one\n<details class=\"quote\"><summary>\u{2026}</summary>&gt;&gt; two\n&gt; &gt; three\n</details>&gt; back\nend"
        );
        assert!(to_html(">> tail").ends_with("</details>"));
    }
}
