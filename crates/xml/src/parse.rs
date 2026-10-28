//! A strict, non-validating XML parser for the subset of XML libvirt emits.

use std::fmt;

use crate::{Element, MAX_DEPTH, MAX_INPUT, Node};

/// Why a document could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset in the input where the problem was found.
    pub offset: usize,
    pub message: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid XML at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

type Result<T> = std::result::Result<T, ParseError>;

/// Parses a document and returns its root element.
///
/// The XML declaration, processing instructions and comments outside the
/// root element are discarded. Whitespace-only text between elements is
/// dropped so that re-serialization produces clean indentation.
pub fn parse(input: &str) -> Result<Element> {
    if input.len() > MAX_INPUT {
        return Err(ParseError { offset: 0, message: "document too large" });
    }
    let mut p = Parser { src: input, pos: 0 };
    p.skip_prolog()?;
    let root = p.element(0)?;
    p.skip_misc()?;
    if p.pos != p.src.len() {
        return p.err("unexpected content after the root element");
    }
    Ok(root)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err<T>(&self, message: &'static str) -> Result<T> {
        Err(ParseError { offset: self.pos, message })
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn starts_with(&self, s: &str) -> bool {
        self.rest().starts_with(s)
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_ws(&mut self) {
        let trimmed = self.rest().trim_start_matches([' ', '\t', '\r', '\n']);
        self.pos = self.src.len() - trimmed.len();
    }

    fn expect(&mut self, s: &str, message: &'static str) -> Result<()> {
        if self.starts_with(s) {
            self.pos += s.len();
            Ok(())
        } else {
            self.err(message)
        }
    }

    /// Advances past `end`, returning the text before it.
    fn take_until(&mut self, end: &str, message: &'static str) -> Result<&'a str> {
        match self.rest().find(end) {
            Some(i) => {
                let s = &self.rest()[..i];
                self.pos += i + end.len();
                Ok(s)
            }
            None => self.err(message),
        }
    }

    /// Skips the XML declaration, comments, PIs and whitespace before the root.
    fn skip_prolog(&mut self) -> Result<()> {
        if self.starts_with("\u{feff}") {
            self.pos += 3;
        }
        self.skip_misc()?;
        if self.starts_with("<!DOCTYPE") {
            return self.err("DTDs are not supported");
        }
        Ok(())
    }

    fn skip_misc(&mut self) -> Result<()> {
        loop {
            self.skip_ws();
            if self.starts_with("<?") {
                self.take_until("?>", "unterminated processing instruction")?;
            } else if self.starts_with("<!--") {
                self.pos += 4;
                self.take_until("-->", "unterminated comment")?;
            } else {
                return Ok(());
            }
        }
    }

    fn name(&mut self) -> Result<&'a str> {
        let rest = self.rest();
        let len = rest.char_indices().find(|&(i, c)| !is_name_char(c, i == 0)).map_or(rest.len(), |(i, _)| i);
        if len == 0 {
            return self.err("expected a name");
        }
        self.pos += len;
        Ok(&rest[..len])
    }

    fn element(&mut self, depth: usize) -> Result<Element> {
        if depth >= MAX_DEPTH {
            return self.err("elements nested too deeply");
        }
        self.expect("<", "expected an element")?;
        let mut el = Element::new(self.name()?);

        // Attributes.
        loop {
            let had_ws = self.peek().is_some_and(|c| c.is_ascii_whitespace());
            self.skip_ws();
            if self.starts_with("/>") {
                self.pos += 2;
                return Ok(el);
            }
            if self.starts_with(">") {
                self.pos += 1;
                break;
            }
            if !had_ws {
                return self.err("expected whitespace before attribute");
            }
            let name = self.name()?;
            self.skip_ws();
            self.expect("=", "expected '=' after attribute name")?;
            self.skip_ws();
            let quote = match self.peek() {
                Some(q @ ('"' | '\'')) => q,
                _ => return self.err("expected a quoted attribute value"),
            };
            self.pos += 1;
            let start = self.pos;
            let raw = self.take_until(if quote == '"' { "\"" } else { "'" }, "unterminated attribute")?;
            if raw.contains('<') {
                self.pos = start;
                return self.err("'<' is not allowed in attribute values");
            }
            let value = unescape(raw, start)?;
            if el.attr(name).is_some() {
                return self.err("duplicate attribute");
            }
            el.attrs.push((name.to_owned(), value));
        }

        // Content.
        loop {
            if self.starts_with("</") {
                self.pos += 2;
                let close = self.name()?;
                if close != el.name {
                    return self.err("mismatched closing tag");
                }
                self.skip_ws();
                self.expect(">", "expected '>'")?;
                return Ok(el);
            } else if self.starts_with("<!--") {
                self.pos += 4;
                let c = self.take_until("-->", "unterminated comment")?;
                el.children.push(Node::Comment(c.to_owned()));
            } else if self.starts_with("<![CDATA[") {
                self.pos += 9;
                let c = self.take_until("]]>", "unterminated CDATA section")?;
                el.children.push(Node::CData(c.to_owned()));
            } else if self.starts_with("<?") {
                self.take_until("?>", "unterminated processing instruction")?;
            } else if self.starts_with("<!") {
                return self.err("unsupported markup declaration");
            } else if self.starts_with("<") {
                let child = self.element(depth + 1)?;
                el.children.push(Node::Element(child));
            } else if self.pos >= self.src.len() {
                return self.err("unexpected end of document");
            } else {
                let start = self.pos;
                let end = self.rest().find('<').map_or(self.src.len(), |i| start + i);
                let raw = &self.src[start..end];
                self.pos = end;
                if !raw.trim().is_empty() {
                    el.children.push(Node::Text(unescape(raw, start)?));
                }
            }
        }
    }
}

fn is_name_char(c: char, first: bool) -> bool {
    c.is_alphabetic() || c == '_' || c == ':' || (!first && (c.is_alphanumeric() || matches!(c, '-' | '.')))
}

/// Resolves the predefined entities and character references in `raw`.
fn unescape(raw: &str, offset: usize) -> Result<String> {
    if !raw.contains('&') {
        return Ok(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after.find(';').filter(|&e| e <= 10).ok_or(ParseError {
            offset: offset + (raw.len() - rest.len()) + i,
            message: "unterminated entity reference",
        })?;
        let entity = &after[..end];
        let ch = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "apos" => Some('\''),
            "quot" => Some('"'),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(std::result::Result::ok)
                .and_then(char::from_u32)
                .filter(|&c| c != '\0'),
        };
        match ch {
            Some(c) => out.push(c),
            None => {
                return Err(ParseError {
                    offset: offset + (raw.len() - rest.len()) + i,
                    message: "unknown entity reference",
                });
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_libvirt_style_document() {
        let xml = r#"<?xml version="1.0"?>
<!-- generated -->
<domain type='kvm' id="3">
  <name>fedora &amp; co</name>
  <metadata>
    <libosinfo:libosinfo xmlns:libosinfo="http://libosinfo.org/xmlns/libvirt/domain/1.0">
      <libosinfo:os id="http://fedoraproject.org/fedora/40"/>
    </libosinfo:libosinfo>
  </metadata>
  <devices><disk type="file" device="disk"><source file="/var/lib/a b.qcow2"/></disk></devices>
</domain>
"#;
        let d = parse(xml).unwrap();
        assert_eq!(d.name, "domain");
        assert_eq!(d.attr("type"), Some("kvm"));
        assert_eq!(d.find_text("name").as_deref(), Some("fedora & co"));
        assert_eq!(
            d.find_attr("metadata/libosinfo:libosinfo/libosinfo:os", "id"),
            Some("http://fedoraproject.org/fedora/40")
        );
        assert_eq!(d.find_attr("devices/disk/source", "file"), Some("/var/lib/a b.qcow2"));
    }

    #[test]
    fn round_trips() {
        let xml = "<a x=\"1 &lt; 2\"><b>t&amp;</b><!-- c --><c/><![CDATA[<raw>]]></a>";
        let d = parse(xml).unwrap();
        let again = parse(&d.to_xml()).unwrap();
        assert_eq!(d, again);
    }

    #[test]
    fn character_references() {
        let d = parse("<a>&#65;&#x42;</a>").unwrap();
        assert_eq!(d.text(), "AB");
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "<!DOCTYPE x [<!ENTITY a 'b'>]><x>&a;</x>",
            "<a>",
            "<a></b>",
            "<a x='1' x='2'/>",
            "<a>&unknown;</a>",
            "<a>&#0;</a>",
            "<a/><b/>",
            "<a x=1/>",
            "<a x='<'/>",
            "",
        ] {
            assert!(parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn limits_depth() {
        let deep = "<a>".repeat(MAX_DEPTH + 1) + &"</a>".repeat(MAX_DEPTH + 1);
        assert!(parse(&deep).is_err());
        let ok = "<a>".repeat(10) + &"</a>".repeat(10);
        assert!(parse(&ok).is_ok());
    }
}
