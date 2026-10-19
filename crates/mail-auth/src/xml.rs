//! A small XML reader for the autoconfig format only (SPEC_GAPS G26). It builds an element
//! tree from well-formed input and rejects anything else; it never fetches external
//! entities or DTDs, so a hostile file can do no more than fail to parse.

/// One element: name, attributes, children and its own text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn children_named<'a, 'n>(
        &'a self,
        name: &'n str,
    ) -> impl Iterator<Item = &'a Element> + use<'a, 'n> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The trimmed text of the first child named `name`.
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.children_named(name).next().map(|c| c.text.trim())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XmlError(pub &'static str);

const MAX_DEPTH: usize = 16;

/// Parses a document and returns its root element.
pub(crate) fn parse(input: &str) -> Result<Element, XmlError> {
    let mut p = Parser {
        s: input.as_bytes(),
        i: 0,
    };
    p.skip_prolog()?;
    let root = p.element(0)?;
    p.skip_misc()?;
    if p.i != p.s.len() {
        return Err(XmlError("content after the root element"));
    }
    Ok(root)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn starts_with(&self, prefix: &str) -> bool {
        self.s[self.i..].starts_with(prefix.as_bytes())
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    /// Advances past the next `end`.
    fn skip_past(&mut self, end: &str) -> Result<(), XmlError> {
        let rest = &self.s[self.i..];
        let pos = rest
            .windows(end.len())
            .position(|w| w == end.as_bytes())
            .ok_or(XmlError("unterminated construct"))?;
        self.i += pos + end.len();
        Ok(())
    }

    /// Comments and processing instructions between elements.
    fn skip_misc(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_ws();
            if self.starts_with("<!--") {
                self.skip_past("-->")?;
            } else if self.starts_with("<?") {
                self.skip_past("?>")?;
            } else {
                return Ok(());
            }
        }
    }

    fn skip_prolog(&mut self) -> Result<(), XmlError> {
        if self.starts_with("\u{feff}") {
            self.i += 3;
        }
        loop {
            self.skip_misc()?;
            if self.starts_with("<!DOCTYPE") {
                // An internal subset could declare entities; refuse rather than expand.
                let rest = &self.s[self.i..];
                let end = rest
                    .iter()
                    .position(|b| *b == b'>')
                    .ok_or(XmlError("unterminated DOCTYPE"))?;
                if rest[..end].contains(&b'[') {
                    return Err(XmlError("DOCTYPE with an internal subset"));
                }
                self.i += end + 1;
            } else {
                return Ok(());
            }
        }
    }

    fn name(&mut self) -> Result<String, XmlError> {
        let start = self.i;
        while self.i < self.s.len() {
            let b = self.s[self.i];
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':') || b >= 0x80 {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            return Err(XmlError("expected a name"));
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn element(&mut self, depth: usize) -> Result<Element, XmlError> {
        if depth > MAX_DEPTH {
            return Err(XmlError("nested too deeply"));
        }
        if !self.starts_with("<") {
            return Err(XmlError("expected an element"));
        }
        self.i += 1;
        let mut el = Element {
            name: self.name()?,
            ..Element::default()
        };
        loop {
            self.skip_ws();
            if self.starts_with("/>") {
                self.i += 2;
                return Ok(el);
            }
            if self.starts_with(">") {
                self.i += 1;
                break;
            }
            let key = self.name()?;
            self.skip_ws();
            if !self.starts_with("=") {
                return Err(XmlError("expected '=' after an attribute name"));
            }
            self.i += 1;
            self.skip_ws();
            let quote = *self.s.get(self.i).ok_or(XmlError("unexpected end"))?;
            if quote != b'"' && quote != b'\'' {
                return Err(XmlError("attribute value must be quoted"));
            }
            self.i += 1;
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != quote {
                self.i += 1;
            }
            if self.i >= self.s.len() {
                return Err(XmlError("unterminated attribute value"));
            }
            let raw = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
            self.i += 1;
            el.attrs.push((key, unescape(&raw)?));
        }
        loop {
            if self.i >= self.s.len() {
                return Err(XmlError("unclosed element"));
            }
            if self.starts_with("</") {
                self.i += 2;
                let name = self.name()?;
                self.skip_ws();
                if name != el.name || !self.starts_with(">") {
                    return Err(XmlError("mismatched closing tag"));
                }
                self.i += 1;
                return Ok(el);
            }
            if self.starts_with("<!--") {
                self.skip_past("-->")?;
            } else if self.starts_with("<![CDATA[") {
                self.i += 9;
                let start = self.i;
                self.skip_past("]]>")?;
                el.text
                    .push_str(&String::from_utf8_lossy(&self.s[start..self.i - 3]));
            } else if self.starts_with("<?") {
                self.skip_past("?>")?;
            } else if self.starts_with("<") {
                let child = self.element(depth + 1)?;
                el.children.push(child);
            } else {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'<' {
                    self.i += 1;
                }
                let raw = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                el.text.push_str(&unescape(&raw)?);
            }
        }
    }
}

/// Replaces the five predefined entities and numeric character references.
fn unescape(raw: &str) -> Result<String, XmlError> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after.find(';').ok_or(XmlError("unterminated entity"))?;
        let entity = &after[..semi];
        let ch = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32)
                    .ok_or(XmlError("unknown entity"))?
            }
        };
        out.push(ch);
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elements_attributes_text_and_entities() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- a comment -->
<!DOCTYPE clientConfig>
<clientConfig version='1.1'>
  <emailProvider id="example.com">
    <displayName>Tom &amp; Jerry &#x263A;&#65;</displayName>
    <empty/>
    <cdata><![CDATA[<raw>]]></cdata>
  </emailProvider>
</clientConfig>
"#;
        let root = parse(doc).unwrap();
        assert_eq!(root.name, "clientConfig");
        assert_eq!(root.attr("version"), Some("1.1"));
        let provider = root.children_named("emailProvider").next().unwrap();
        assert_eq!(provider.attr("id"), Some("example.com"));
        assert_eq!(provider.child_text("displayName"), Some("Tom & Jerry ☺A"));
        assert_eq!(provider.child_text("empty"), Some(""));
        assert_eq!(provider.child_text("cdata"), Some("<raw>"));
    }

    #[test]
    fn rejects_malformed_and_hostile_input() {
        for bad in [
            "",
            "text",
            "<a>",
            "<a></b>",
            "<a x=1></a>",
            "<a>&bogus;</a>",
            "<a></a><b></b>",
            "<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = "<a>".repeat(40) + &"</a>".repeat(40);
        assert!(parse(&deep).is_err());
    }
}
