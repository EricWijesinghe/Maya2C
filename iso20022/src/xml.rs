//! Reading and writing the XML, with every bound in one place.
//!
//! ## Why a bounded tree and not a streaming walk
//!
//! Each message type here needs to look at elements out of document order — a
//! statement's closing balance is decided by entries that appear after it — so
//! a streaming parser would mean every message keeping its own partial state
//! and its own limits. Instead the document becomes a small tree **once**, with
//! every bound checked during that one pass, and the message types walk a
//! structure that is already known to be finite.
//!
//! The tree costs memory proportional to the document, which is exactly why
//! [`MAX_DOCUMENT_BYTES`] is checked before the first byte is parsed rather
//! than after.
//!
//! ## What this parser refuses to process at all
//!
//! A bank rail is where an XXE fetch and an entity-expansion bomb arrive, so
//! both are refused structurally rather than bounded:
//!
//! - **No DTD.** A `<!DOCTYPE …>` is an error, not an ignored node. Without a
//!   doctype there is nowhere to declare an entity, so entity expansion is not
//!   something this crate limits — it is something the document cannot express.
//!   quick-xml expands nothing but the five predefined entities on its own, and
//!   a reference to any other — `&xxe;` — fails unescaping and arrives here as
//!   [`Error::Xml`] rather than as an empty string, which is the reading that
//!   would let a payload smuggle a field past a check by naming an entity
//!   nobody declared.
//! - **No processing instructions.** Nothing here acts on one, and a parser
//!   that silently drops what it does not understand is a parser two
//!   implementations disagree about.
//! - **No external anything.** This crate opens no files and makes no network
//!   calls; there is no resolver to point at a URL.
//!
//! Depth, breadth, text length and attribute count are bounded because those
//! cost memory and time without needing a doctype at all: ten thousand nested
//! `<a>` elements is a stack overflow in a recursive walker, and this one is
//! iterative for the same reason.
//!
//! ## Namespaces
//!
//! Matching is on the **local** name. `<Document>`, `<urn:Document>` and
//! `<ns0:Document>` are the same element, because the prefix is a serializer's
//! choice and every counterparty makes a different one. The namespace URI is
//! checked once, at the document root, where it identifies the message version.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

use crate::error::{Error, Result};

/// The largest document this crate will parse.
///
/// A pacs.008 carrying the maximum number of transactions this bridge accepts
/// is a few hundred kilobytes. One megabyte is generous for that and small
/// enough that a hostile sender cannot make memory the attack.
pub const MAX_DOCUMENT_BYTES: usize = 1 << 20;

/// The deepest element nesting.
///
/// The deepest legitimate path in the three messages implemented here is about
/// a dozen (`Document/BkToCstmrStmt/Stmt/Ntry/NtryDtls/TxDtls/RltdPties/Dbtr/…`).
/// Thirty-two leaves room for a counterparty's optional wrappers without
/// leaving room for a nesting bomb.
pub const MAX_DEPTH: usize = 32;

/// The most children one element may have.
///
/// This is what bounds the transaction count of a pacs.008 and the entry count
/// of a camt.053 — both are repeated children of a single parent.
pub const MAX_CHILDREN: usize = 4_096;

/// The longest text a single element may carry.
pub const MAX_TEXT_BYTES: usize = 1_024;

/// The most attributes one element may carry.
pub const MAX_ATTRIBUTES: usize = 16;

/// One element of a parsed document.
///
/// Text and children are both kept: ISO 20022 elements are either containers or
/// leaves, never both, but refusing mixed content here would mean the parser
/// deciding which a counterparty's extension element is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    /// Local name, with any namespace prefix stripped.
    pub name: String,
    /// Attributes, in document order, by local name.
    pub attributes: Vec<(String, String)>,
    /// Text content, with the five predefined entities unescaped.
    pub text: String,
    /// Child elements, in document order.
    pub children: Vec<Element>,
}

impl Element {
    /// A new, empty element.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attributes: Vec::new(),
            text: String::new(),
            children: Vec::new(),
        }
    }

    /// A leaf element carrying `text`.
    #[must_use]
    pub fn leaf(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::new(name)
        }
    }

    /// Adds a child and returns `self`, so a message can be built in one
    /// expression.
    #[must_use]
    pub fn with(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    /// Adds a child only when there is one, for the optional half of a schema.
    #[must_use]
    pub fn maybe(self, child: Option<Element>) -> Self {
        match child {
            Some(child) => self.with(child),
            None => self,
        }
    }

    /// Adds an attribute and returns `self`.
    #[must_use]
    pub fn attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.push((name.into(), value.into()));
        self
    }

    /// The first child with this local name, if any.
    #[must_use]
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The first child with this local name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Missing`] if there is none.
    pub fn require(&self, name: &'static str) -> Result<&Element> {
        self.child(name).ok_or(Error::Missing(name))
    }

    /// Every child with this local name, in document order.
    pub fn all(&self, name: &str) -> impl Iterator<Item = &Element> {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The text of the first child with this local name, if any.
    #[must_use]
    pub fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).map(|child| child.text.as_str())
    }

    /// The text of the first child with this local name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Missing`] if there is none.
    pub fn require_text(&self, name: &'static str) -> Result<&str> {
        Ok(self.require(name)?.text.as_str())
    }

    /// An attribute's value by local name, if present.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Parses a document into its root element.
///
/// # Errors
///
/// Returns [`Error::Bound`] if the document, its nesting, an element's children,
/// its text or its attribute count exceed the limits above; [`Error::Xml`] for
/// bytes that are not well-formed UTF-8 XML, for a DOCTYPE, or for a processing
/// instruction.
pub fn parse(bytes: &[u8]) -> Result<Element> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(Error::Bound {
            what: "document",
            found: bytes.len(),
            limit: MAX_DOCUMENT_BYTES,
        });
    }
    // Up front, and for the whole document: ISO 20022 is UTF-8, and a parser
    // that validated encoding per-element would have already allocated for the
    // elements before the first bad byte.
    let text = std::str::from_utf8(bytes)
        .map_err(|error| Error::Xml(format!("not valid UTF-8: {error}")))?;

    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    reader.config_mut().expand_empty_elements = false;
    reader.config_mut().check_end_names = true;

    // An explicit stack rather than recursion: the depth bound then protects
    // memory rather than the call stack, and a document one element past the
    // bound is a clean error instead of an abort.
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    loop {
        match reader
            .read_event()
            .map_err(|error| Error::Xml(error.to_string()))?
        {
            Event::Start(start) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(Error::Bound {
                        what: "element nesting",
                        found: stack.len() + 1,
                        limit: MAX_DEPTH,
                    });
                }
                stack.push(element_from(&start)?);
            }
            Event::Empty(start) => {
                let element = element_from(&start)?;
                close(&mut stack, &mut root, element)?;
            }
            Event::End(end) => {
                let element = stack.pop().ok_or_else(|| {
                    Error::Xml(format!("</{}> closes nothing", local_name(end.name().0)))
                })?;
                close(&mut stack, &mut root, element)?;
            }
            Event::Text(text) => append_text(&mut stack, &text)?,
            Event::CData(data) => {
                // CDATA is text that skipped escaping, so it is the same value
                // to this crate — but it must still be bounded, or it would be
                // the one way past `MAX_TEXT_BYTES`.
                let unescaped = data
                    .minimal_escape()
                    .map_err(|error| Error::Xml(error.to_string()))?;
                append_text(&mut stack, &unescaped)?;
            }
            // An XML declaration is the only prologue this crate reads, and it
            // carries nothing it acts on. Comments are dropped: they are not
            // data, and no ISO 20022 field lives in one.
            Event::Decl(_) | Event::Comment(_) => {}
            Event::DocType(_) => {
                return Err(Error::Xml(
                    "a DOCTYPE is refused: it is where an entity would be declared, \
                     and this crate has no use for one"
                        .into(),
                ));
            }
            Event::PI(_) => {
                return Err(Error::Xml(
                    "a processing instruction is refused: nothing here acts on one".into(),
                ));
            }
            Event::Eof => break,
        }
    }

    if !stack.is_empty() {
        return Err(Error::Xml(format!(
            "{} element(s) were never closed",
            stack.len()
        )));
    }
    root.ok_or_else(|| Error::Xml("document has no root element".into()))
}

/// Finishes `element`: into its parent, or into the root slot.
fn close(stack: &mut [Element], root: &mut Option<Element>, element: Element) -> Result<()> {
    match stack.last_mut() {
        Some(parent) => {
            if parent.children.len() >= MAX_CHILDREN {
                return Err(Error::Bound {
                    what: "child elements",
                    found: parent.children.len() + 1,
                    limit: MAX_CHILDREN,
                });
            }
            parent.children.push(element);
            Ok(())
        }
        None if root.is_none() => {
            *root = Some(element);
            Ok(())
        }
        // Two roots is not well-formed XML, and quick-xml will usually catch it
        // first; this is here so the invariant holds whatever the reader does.
        None => Err(Error::Xml("document has a second root element".into())),
    }
}

/// Appends text to the element currently open, bounded.
fn append_text(stack: &mut [Element], text: &BytesText<'_>) -> Result<()> {
    let Some(current) = stack.last_mut() else {
        // Text outside the root is whitespace at worst, and trim_text has
        // already dropped that.
        return Ok(());
    };
    let unescaped = text
        .unescape()
        .map_err(|error| Error::Xml(error.to_string()))?;
    if current.text.len() + unescaped.len() > MAX_TEXT_BYTES {
        return Err(Error::Bound {
            what: "element text",
            found: current.text.len() + unescaped.len(),
            limit: MAX_TEXT_BYTES,
        });
    }
    current.text.push_str(&unescaped);
    Ok(())
}

/// An [`Element`] from a start tag, with its attributes.
fn element_from(start: &BytesStart<'_>) -> Result<Element> {
    let mut element = Element::new(local_name(start.name().0));
    for attribute in start.attributes() {
        if element.attributes.len() >= MAX_ATTRIBUTES {
            return Err(Error::Bound {
                what: "attributes",
                found: element.attributes.len() + 1,
                limit: MAX_ATTRIBUTES,
            });
        }
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        let value = attribute
            .unescape_value()
            .map_err(|error| Error::Xml(error.to_string()))?;
        if value.len() > MAX_TEXT_BYTES {
            return Err(Error::Bound {
                what: "attribute value",
                found: value.len(),
                limit: MAX_TEXT_BYTES,
            });
        }
        element
            .attributes
            .push((local_name(attribute.key.0), value.into_owned()));
    }
    Ok(element)
}

/// A qualified name with any prefix stripped.
fn local_name(qualified: &[u8]) -> String {
    let local = match qualified.iter().position(|byte| *byte == b':') {
        Some(colon) => &qualified[colon + 1..],
        None => qualified,
    };
    String::from_utf8_lossy(local).into_owned()
}

/// Renders an element tree, with an XML declaration and no whitespace.
///
/// No pretty-printing, deliberately. Indentation is significant to nobody here
/// and would be a second representation of the same document; a counterparty
/// that wants it can format what it receives.
///
/// # Errors
///
/// Returns [`Error::Xml`] if the writer fails, which for an in-memory buffer
/// means an element name that cannot be written.
pub fn render(root: &Element, namespace: &str) -> Result<String> {
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Decl(quick_xml::events::BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            None,
        )))
        .map_err(|error| Error::Xml(error.to_string()))?;

    let mut root = root.clone();
    root.attributes
        .insert(0, ("xmlns".into(), namespace.into()));
    write_element(&mut writer, &root)?;

    String::from_utf8(writer.into_inner()).map_err(|error| Error::Xml(error.to_string()))
}

/// Writes one element and its subtree.
fn write_element(writer: &mut Writer<Vec<u8>>, element: &Element) -> Result<()> {
    let mut start = BytesStart::new(element.name.as_str());
    for (name, value) in &element.attributes {
        start.push_attribute((name.as_str(), value.as_str()));
    }
    writer
        .write_event(Event::Start(start))
        .map_err(|error| Error::Xml(error.to_string()))?;

    if !element.text.is_empty() {
        writer
            .write_event(Event::Text(BytesText::new(element.text.as_str())))
            .map_err(|error| Error::Xml(error.to_string()))?;
    }
    for child in &element.children {
        write_element(writer, child)?;
    }

    writer
        .write_event(Event::End(BytesEnd::new(element.name.as_str())))
        .map_err(|error| Error::Xml(error.to_string()))
}
