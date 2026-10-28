//! Serialization of [`Element`] trees.

use std::borrow::Cow;

use crate::{Element, Node};

pub(crate) fn escape(s: &str) -> Cow<'_, str> {
    if !s.contains(['<', '>', '&', '"', '\'']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

pub(crate) fn write_element(out: &mut String, el: &Element, level: usize) {
    indent(out, level);
    out.push('<');
    out.push_str(&el.name);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape(v));
        out.push('"');
    }
    if el.children.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push('>');

    // Elements holding only text stay on one line: <name>foo</name>.
    let only_text = el.children.iter().all(|n| matches!(n, Node::Text(_) | Node::CData(_)));
    if only_text {
        // Text is written verbatim here so values such as descriptions keep
        // their exact whitespace.
        for n in &el.children {
            match n {
                Node::Text(t) => out.push_str(&escape(t)),
                other => write_inline(out, other),
            }
        }
    } else {
        out.push('\n');
        for n in &el.children {
            match n {
                Node::Element(child) => write_element(out, child, level + 1),
                other => {
                    indent(out, level + 1);
                    write_inline(out, other);
                    out.push('\n');
                }
            }
        }
        indent(out, level);
    }
    out.push_str("</");
    out.push_str(&el.name);
    out.push_str(">\n");
}

fn write_inline(out: &mut String, n: &Node) {
    match n {
        Node::Text(t) => out.push_str(&escape(t.trim())),
        Node::CData(t) => {
            // "]]>" cannot appear inside CDATA; split it across two sections.
            out.push_str("<![CDATA[");
            out.push_str(&t.replace("]]>", "]]]]><![CDATA[>"));
            out.push_str("]]>");
        }
        Node::Comment(t) => {
            out.push_str("<!--");
            out.push_str(&t.replace("--", "- -"));
            out.push_str("-->");
        }
        Node::Element(e) => write_element(out, e, 0),
    }
}
