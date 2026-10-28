//! A small XML document model for libvirt XML.
//!
//! libvirt describes every object (domains, networks, storage pools, …) as an
//! XML document. Editing configuration safely means parsing that document,
//! changing a few nodes and writing it back *without losing anything we do not
//! understand*. This crate provides exactly that: a lossless-enough element
//! tree (elements, attributes, text, comments and CDATA are preserved, in
//! order) with convenient lookup and mutation helpers.
//!
//! The parser deliberately supports only what libvirt produces:
//!
//! * no DTDs: a `<!DOCTYPE …>` is rejected, so entity expansion attacks are
//!   impossible;
//! * only the five predefined entities and numeric character references;
//! * a hard nesting-depth and input-size limit.

mod parse;
mod write;

pub use parse::{ParseError, parse};

use std::fmt;

/// Maximum element nesting accepted by the parser.
pub const MAX_DEPTH: usize = 256;
/// Maximum input size accepted by the parser (16 MiB).
pub const MAX_INPUT: usize = 16 * 1024 * 1024;

/// A node inside an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Text(String),
    Comment(String),
    CData(String),
}

/// An XML element with its attributes (in document order) and children.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    /// Creates an empty element.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), attrs: Vec::new(), children: Vec::new() }
    }

    /// Builder-style attribute setter.
    #[must_use]
    pub fn with_attr(mut self, name: &str, value: impl Into<String>) -> Self {
        self.set_attr(name, value);
        self
    }

    /// Builder-style text setter.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.set_text(text);
        self
    }

    /// Builder-style child appender.
    #[must_use]
    pub fn with_child(mut self, child: Element) -> Self {
        self.children.push(Node::Element(child));
        self
    }

    /// Returns the value of an attribute.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Sets (or replaces) an attribute, keeping its position if it existed.
    pub fn set_attr(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self.attrs.iter_mut().find(|(k, _)| k == name) {
            Some((_, v)) => *v = value,
            None => self.attrs.push((name.to_owned(), value)),
        }
    }

    /// Removes an attribute, returning its old value.
    pub fn remove_attr(&mut self, name: &str) -> Option<String> {
        let pos = self.attrs.iter().position(|(k, _)| k == name)?;
        Some(self.attrs.remove(pos).1)
    }

    /// Iterates over the child elements.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// Iterates mutably over the child elements.
    pub fn elements_mut(&mut self) -> impl Iterator<Item = &mut Element> {
        self.children.iter_mut().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// Iterates over the child elements with the given name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.elements().filter(move |e| e.name == name)
    }

    /// Returns the first child element with the given name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    /// Returns the first child element with the given name, mutably.
    pub fn child_mut(&mut self, name: &str) -> Option<&mut Element> {
        self.elements_mut().find(|e| e.name == name)
    }

    /// Returns the first child with the given name, creating it if missing.
    pub fn child_or_insert(&mut self, name: &str) -> &mut Element {
        let pos = self.children.iter().position(|n| matches!(n, Node::Element(e) if e.name == name));
        let pos = pos.unwrap_or_else(|| {
            self.children.push(Node::Element(Element::new(name)));
            self.children.len() - 1
        });
        match &mut self.children[pos] {
            Node::Element(e) => e,
            _ => unreachable!("position points at an element"),
        }
    }

    /// Follows a `/`-separated path of child element names.
    ///
    /// `dom.find("devices/graphics")` returns the first `<graphics>` inside
    /// the first `<devices>`.
    pub fn find(&self, path: &str) -> Option<&Element> {
        path.split('/').filter(|s| !s.is_empty()).try_fold(self, |e, seg| e.child(seg))
    }

    /// Mutable variant of [`Element::find`].
    pub fn find_mut(&mut self, path: &str) -> Option<&mut Element> {
        path.split('/').filter(|s| !s.is_empty()).try_fold(self, |e, seg| e.child_mut(seg))
    }

    /// Returns the attribute `attr` of the element at `path`.
    pub fn find_attr(&self, path: &str, attr: &str) -> Option<&str> {
        self.find(path)?.attr(attr)
    }

    /// Returns the trimmed text of the element at `path`.
    pub fn find_text(&self, path: &str) -> Option<String> {
        self.find(path).map(Element::text)
    }

    /// Concatenated text and CDATA content of this element (not descendants),
    /// with surrounding whitespace trimmed.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for n in &self.children {
            if let Node::Text(t) | Node::CData(t) = n {
                out.push_str(t);
            }
        }
        out.trim().to_owned()
    }

    /// Replaces all text content of this element. Child elements are kept.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.children.retain(|n| !matches!(n, Node::Text(_) | Node::CData(_)));
        self.children.insert(0, Node::Text(text.into()));
    }

    /// Appends a child element and returns a mutable reference to it.
    pub fn push(&mut self, child: Element) -> &mut Element {
        self.children.push(Node::Element(child));
        match self.children.last_mut() {
            Some(Node::Element(e)) => e,
            _ => unreachable!("just pushed an element"),
        }
    }

    /// Removes every child element for which `pred` returns true.
    /// Returns how many were removed.
    pub fn remove_children(&mut self, mut pred: impl FnMut(&Element) -> bool) -> usize {
        let before = self.children.len();
        self.children.retain(|n| !matches!(n, Node::Element(e) if pred(e)));
        before - self.children.len()
    }

    /// Serializes the element as an indented XML document fragment.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        write::write_element(&mut out, self, 0);
        out
    }
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_xml())
    }
}

/// Escapes text for use inside element content or attribute values.
pub fn escape(s: &str) -> std::borrow::Cow<'_, str> {
    write::escape(s)
}
