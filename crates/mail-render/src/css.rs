//! CSS in mail (section 8, steps 1 and 2): `style` attributes and `<style>` elements are
//! kept, every `url()` is rewritten, and `@import` is disabled. The CSP stays the backstop
//! for anything this scanner does not recognize, such as escaped function names: it allows
//! no style or image source other than the app's own protocols.

/// Rewrites every `url(...)` with `map` (a `None` result empties it) and disables `@import`.
pub(crate) fn rewrite(css: &str, map: &dyn Fn(&str) -> Option<String>) -> String {
    let plain = unescape_names(css);
    let disabled = disable_imports(&plain);
    let strings = rewrite_strings(&disabled, map);
    let css = strings.as_str();
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while let Some(found) = lower[i..].find("url(") {
        let start = i + found;
        out.push_str(&css[i..start]);
        let args = start + 4;
        let Some((value, end)) = url_argument(css, args) else {
            // An unterminated url( swallows the rest of the declaration.
            out.push_str("url()");
            return out;
        };
        out.push_str("url(\"");
        if let Some(new) = map(value.trim()) {
            for c in new.chars() {
                if matches!(c, '"' | '\\' | '\n' | '\r') {
                    continue;
                }
                out.push(c);
            }
        }
        out.push_str("\")");
        i = end;
    }
    out.push_str(&css[i..]);
    out
}

/// The argument of `url(` starting at `from`, and the index just past its `)`.
fn url_argument(css: &str, from: usize) -> Option<(String, usize)> {
    let rest = &css[from..];
    let trimmed = rest.trim_start();
    let offset = from + (rest.len() - trimmed.len());
    let mut chars = trimmed.char_indices();
    match chars.next() {
        Some((_, q @ ('"' | '\''))) => {
            let mut value = String::new();
            let mut escaped = false;
            for (idx, c) in chars.by_ref() {
                if escaped {
                    value.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    let after = offset + idx + c.len_utf8();
                    let close = css[after..].find(')')?;
                    return Some((value, after + close + 1));
                } else {
                    value.push(c);
                }
            }
            None
        }
        _ => {
            let close = trimmed.find(')')?;
            Some((trimmed[..close].to_string(), offset + close + 1))
        }
    }
}

/// Resolves CSS escapes that spell name characters (letters, digits, `-`, `_`, `(`, `@`,
/// `:`), so `\75 rl(` and `@\69mport` are seen as `url(` and `@import`. Other escapes, such
/// as an escaped quote inside a string, stay as written.
fn unescape_names(css: &str) -> String {
    if !css.contains('\\') {
        return css.to_string();
    }
    let name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '(' | '@' | ':');
    let chars: Vec<char> = css.chars().collect();
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let hex: String = chars[i + 1..]
            .iter()
            .take(6)
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if !hex.is_empty() {
            let decoded = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
            let mut end = i + 1 + hex.len();
            if chars.get(end).is_some_and(|c| c.is_whitespace()) {
                end += 1;
            }
            match decoded {
                Some(c) if name_char(c) => out.push(c),
                _ => out.extend(&chars[i..end]),
            }
            i = end;
        } else if let Some(&c) = chars.get(i + 1).filter(|c| name_char(**c)) {
            out.push(c);
            i += 2;
        } else {
            out.push('\\');
            i += 1;
        }
    }
    out
}

/// Quoted strings that are URLs outside `url()`, as `image-set()` and `@font-face src`
/// accept them, go through `map` too. A protocol-relative `//host` string counts as https.
fn rewrite_strings(css: &str, map: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(css.len());
    let mut chars = css.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        if c != '"' && c != '\'' {
            out.push(c);
            continue;
        }
        let mut value = String::new();
        let mut raw_end = css.len();
        let mut escaped = false;
        for (j, d) in chars.by_ref() {
            if escaped {
                value.push(d);
                escaped = false;
            } else if d == '\\' {
                escaped = true;
            } else if d == c {
                raw_end = j + d.len_utf8();
                break;
            } else {
                value.push(d);
            }
        }
        let raw = &css[idx..raw_end];
        let in_url = out.trim_end().to_ascii_lowercase().ends_with("url(");
        let lower = value.trim().to_ascii_lowercase();
        let url = if lower.starts_with("//") {
            Some(format!("https:{}", value.trim()))
        } else if lower.starts_with("http:") || lower.starts_with("https:") {
            Some(value.trim().to_string())
        } else {
            None
        };
        match url.filter(|_| !in_url) {
            Some(url) => {
                out.push(c);
                if let Some(new) = map(&url) {
                    out.extend(
                        new.chars()
                            .filter(|ch| !matches!(ch, '"' | '\'' | '\\' | '\n' | '\r')),
                    );
                }
                out.push(c);
            }
            None => out.push_str(raw),
        }
    }
    out
}

/// `@import` loads a stylesheet: each rule is removed up to its `;` (or the end).
fn disable_imports(css: &str) -> String {
    let lower = css.to_ascii_lowercase();
    if !lower.contains("@import") {
        return disable_legacy(css);
    }
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while let Some(found) = lower[i..].find("@import") {
        let at = i + found;
        out.push_str(&css[i..at]);
        i = lower[at..].find(';').map_or(css.len(), |end| at + end + 1);
    }
    out.push_str(&css[i..]);
    disable_legacy(&out)
}

/// Old engines ran code from `expression()`, `behavior` and `-moz-binding`; none of today's
/// webviews do, but the names are broken anyway so no value ever reaches one.
fn disable_legacy(css: &str) -> String {
    const LEGACY: &[&str] = &["expression(", "behavior", "-moz-binding"];
    let lower = css.to_ascii_lowercase();
    if !LEGACY.iter().any(|t| lower.contains(t)) {
        return css.to_string();
    }
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while i < css.len() {
        match LEGACY.iter().find(|t| lower[i..].starts_with(**t)) {
            Some(t) => {
                out.push_str("zeromail-blocked-");
                out.push_str(&t.replace('(', "-("));
                i += t.len();
            }
            None => {
                let c = css[i..].chars().next().unwrap_or(' ');
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(url: &str) -> Option<String> {
        url.starts_with("http").then(|| format!("R[{url}]"))
    }

    #[test]
    fn urls_are_rewritten_and_imports_disabled() {
        assert_eq!(
            rewrite("background:URL( 'http://x.test/a.png' ) no-repeat", &map),
            "background:url(\"R[http://x.test/a.png]\") no-repeat"
        );
        assert_eq!(
            rewrite("a{b:url(http://x/1)}c{d:url(\"javascript:x\")}", &map),
            "a{b:url(\"R[http://x/1]\")}c{d:url(\"\")}"
        );
        assert_eq!(
            rewrite("@IMPORT url(http://evil.test/x.css); p{}", &map),
            " p{}"
        );
        assert_eq!(rewrite("a{} @import 'x.css'", &map), "a{} ");
        assert_eq!(
            rewrite("x:url(\"http://a\\\"b\")", &map),
            "x:url(\"R[http://a\"b]\")".replace("a\"b", "ab")
        );
        assert_eq!(rewrite("x:url(http://never-closed", &map), "x:url()");
        assert_eq!(rewrite("color:red", &map), "color:red");
        assert_eq!(
            rewrite("x:\\75 rl(http://e/1)", &map),
            "x:url(\"R[http://e/1]\")"
        );
        assert_eq!(rewrite("@\\69mport 'http://e/x.css'; a{}", &map), " a{}");
        assert_eq!(rewrite("q:\"\\201C\\\"\"", &map), "q:\"\\201C\\\"\"");
        assert_eq!(
            rewrite("b:image-set('http://e/a.png' 1x, \"//e/b.png\" 2x); c:url('http://e/c')", &map),
            "b:image-set('R[http://e/a.png]' 1x, \"R[https://e/b.png]\" 2x); c:url(\"R[http://e/c]\")"
        );
        assert_eq!(
            rewrite("content:'plain text'", &map),
            "content:'plain text'"
        );
        assert_eq!(
            rewrite("width:EXPRESSION(alert(1))", &map),
            "width:zeromail-blocked-expression-(alert(1))"
        );
    }
}
