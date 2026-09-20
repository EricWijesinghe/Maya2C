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

use quick_xml::events::{BytesEnd, BytesRef, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};

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

    // No `trim_text`: see `append_text` for why whitespace is trimmed per
    // element rather than per event.
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = false;
    reader.config_mut().check_end_names = true;

    // An explicit stack rather than recursion: the depth bound then protects
    // memory rather than the call stack, and a document one element past the
    // bound is a clean error instead of an abort.
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut gap = Gap::default();

    loop {
        match reader
            .read_event()
            .map_err(|error| Error::Xml(error.to_string()))?
        {
            Event::Start(start) => {
                gap.markup();
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
                gap.markup();
                let element = element_from(&start)?;
                close(&mut stack, &mut root, element)?;
            }
            Event::End(end) => {
                gap.markup();
                let element = stack.pop().ok_or_else(|| {
                    Error::Xml(format!("</{}> closes nothing", shown(end.name().0)))
                })?;
                close(&mut stack, &mut root, element)?;
            }
            Event::Text(text) => {
                append_text(&mut stack, &mut gap, &text.xml10_content(), Edges::Trim)?;
            }
            // A reference arrives as its own event. What it names is content,
            // whitespace or not: `&#32;` is a space somebody wrote on purpose.
            Event::GeneralRef(reference) => {
                let resolved = resolve_reference(&reference)?;
                append_text(&mut stack, &mut gap, &resolved, Edges::Keep)?;
            }
            Event::CData(data) => {
                // CDATA is text that skipped escaping, so it is the same value
                // to this crate — but it must still be bounded, or it would be
                // the one way past `MAX_TEXT_BYTES`.
                append_text(&mut stack, &mut gap, &data.xml10_content(), Edges::Keep)?;
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

/// Whether a chunk's edge whitespace may be dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edges {
    /// Ordinary text between markup.
    Trim,
    /// A resolved reference or CDATA: exactly what the author wrote.
    Keep,
}

/// Longest reference name accepted between `&` and `;`. The longest
/// predefined entity is four bytes; this leaves room for a character reference
/// with a few leading zeros and no room for a name used as a payload.
const MAX_REFERENCE_BYTES: usize = 16;

/// Characters XML counts as whitespace.
const fn is_xml_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn text_bound(found: usize) -> Error {
    Error::Bound {
        what: "element text",
        found,
        limit: MAX_TEXT_BYTES,
    }
}

/// Whitespace state for the run of text between two tags.
#[derive(Default)]
struct Gap {
    /// Whitespace after the last content, held until more content in the same
    /// run shows it was interior.
    pending: String,
    /// No content yet since the last tag opened or closed, so whitespace now is
    /// at the run's leading edge.
    after_markup: bool,
}

impl Gap {
    /// A tag opened or closed: the run of text ends here.
    fn markup(&mut self) {
        self.pending.clear();
        self.after_markup = true;
    }
}

/// Appends a chunk of text to the element currently open, bounded.
///
/// Whitespace at the edges of each run of text between tags is dropped,
/// exactly as `trim_text` did per text event before quick-xml 0.38. It cannot
/// be done per event any more: a reference now arrives as its own event, so
/// `a &amp; b` is `"a "`, `amp`, `" b"`, and trimming each piece would give
/// `a&b`. So leading whitespace is dropped only right after a tag, and trailing
/// whitespace waits in [`Gap::pending`], written only if more content follows
/// before the next tag.
fn append_text(stack: &mut [Element], gap: &mut Gap, chunk: &str, edges: Edges) -> Result<()> {
    let Some(current) = stack.last_mut() else {
        // Outside the root: no element to hold it, so no field could be
        // read from it. Ignored, as it always was.
        return Ok(());
    };
    let (leading, body, trailing) = match edges {
        Edges::Keep => ("", chunk, ""),
        Edges::Trim => {
            let start = chunk.len() - chunk.trim_start_matches(is_xml_whitespace).len();
            let body = chunk[start..].trim_end_matches(is_xml_whitespace);
            (&chunk[..start], body, &chunk[start + body.len()..])
        }
    };
    let at_edge = gap.after_markup || current.text.is_empty();
    if body.is_empty() {
        if !at_edge {
            if gap.pending.len() + chunk.len() > MAX_TEXT_BYTES {
                return Err(text_bound(
                    current.text.len() + gap.pending.len() + chunk.len(),
                ));
            }
            gap.pending.push_str(chunk);
        }
        return Ok(());
    }
    let interior = if at_edge { "" } else { leading };
    let found = current.text.len() + gap.pending.len() + interior.len() + body.len();
    if found > MAX_TEXT_BYTES {
        return Err(text_bound(found));
    }
    current.text.push_str(&gap.pending);
    current.text.push_str(interior);
    current.text.push_str(body);
    gap.pending.clear();
    gap.pending.push_str(trailing);
    gap.after_markup = false;
    Ok(())
}

/// What a reference between `&` and `;` stands for: one of the five
/// predefined entities or a character reference. Anything else is refused —
/// there is no DTD, so there is nothing else it could name.
fn resolve_reference(reference: &BytesRef<'_>) -> Result<String> {
    let name: &str = reference;
    if name.len() > MAX_REFERENCE_BYTES {
        return Err(Error::Xml(format!(
            "an entity reference longer than {MAX_REFERENCE_BYTES} bytes"
        )));
    }
    quick_xml::escape::unescape(&format!("&{name};"))
        .map(|value| value.into_owned())
        .map_err(|error| Error::Xml(error.to_string()))
}

/// An [`Element`] from a start tag, with its attributes.
fn element_from(start: &BytesStart<'_>) -> Result<Element> {
    let mut element = Element::new(checked_local_name(start.name().0)?);
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
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|error| Error::Xml(error.to_string()))?;
        if value.len() > MAX_TEXT_BYTES {
            return Err(Error::Bound {
                what: "attribute value",
                found: value.len(),
                limit: MAX_TEXT_BYTES,
            });
        }
        let name = checked_local_name(attribute.key.0)?;
        // Two prefixes, one local name: `attribute(name)` could answer with only
        // one of them, and which one would be an accident of order.
        if element
            .attributes
            .iter()
            .any(|(existing, _)| *existing == name)
        {
            return Err(Error::Xml(format!(
                "attribute {} appears twice once prefixes are stripped",
                shown(&name)
            )));
        }
        element.attributes.push((name, value.into_owned()));
    }
    Ok(element)
}

/// Longest element or attribute name accepted, prefix included.
const MAX_NAME_BYTES: usize = 128;

/// The local part of a qualified name, or a refusal if it is not a name this
/// crate could write back out.
///
/// quick-xml's reader does not validate names: it reads `<a'b>` as an element
/// called `a'b`, which no writer can render into XML that parses again. Found
/// by `fuzz/fuzz_targets/iso20022_decode.rs`. ISO 20022 names are ASCII, so a
/// name is accepted only as `[prefix:]local`, each part a letter or `_`
/// followed by letters, digits, `_`, `-` or `.`.
fn checked_local_name(qualified: &str) -> Result<String> {
    let valid = |part: &str| {
        let mut chars = part.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    let (prefix, local) = match qualified.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, qualified),
    };
    if qualified.len() > MAX_NAME_BYTES || !valid(local) || prefix.is_some_and(|p| !valid(p)) {
        return Err(Error::Xml(format!(
            "{:?} is not an XML name this reader accepts",
            shown(qualified)
        )));
    }
    Ok(local.to_string())
}

/// A name cut short for an error message, so a refusal cannot echo a megabyte.
fn shown(name: &str) -> String {
    name.chars().take(32).collect()
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
    // The namespace is the caller's to state. One the tree already carries —
    // from a parsed document — would otherwise be written twice, and a
    // duplicate attribute does not parse.
    root.attributes.retain(|(name, _)| name != "xmlns");
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn text_of(document: &str) -> String {
        parse(document.as_bytes()).expect("parses").text
    }

    #[test]
    fn references_keep_the_spaces_beside_them() {
        assert_eq!(text_of("<a>  Smith &amp; Sons  </a>"), "Smith & Sons");
        assert_eq!(text_of("<a>&lt;&#32;&gt;</a>"), "< >");
        assert_eq!(text_of("<a>A&#x42;C</a>"), "ABC");
    }

    #[test]
    fn each_run_of_text_between_tags_is_trimmed_as_before() {
        // What `trim_text` produced per event on quick-xml 0.37.
        assert_eq!(text_of("<a>hi<b/>  more</a>"), "himore");
        assert_eq!(text_of("<a> x <b/> &amp; y </a>"), "x& y");
    }

    #[test]
    fn whitespace_between_children_is_not_text() {
        let root = parse(b"<a>\n  <b> x </b>\n  <c/>\n</a>").expect("parses");
        assert_eq!(root.text, "");
        assert_eq!(root.children[0].text, "x");
        assert_eq!(root.children.len(), 2);
    }

    #[test]
    fn cdata_is_kept_verbatim_and_still_bounded() {
        assert_eq!(text_of("<a><![CDATA[ <b>&amp; ]]></a>"), " <b>&amp; ");
        let long = format!("<a><![CDATA[{}]]></a>", "x".repeat(MAX_TEXT_BYTES + 1));
        assert!(matches!(parse(long.as_bytes()), Err(Error::Bound { .. })));
    }

    #[test]
    fn references_cannot_smuggle_text_past_the_bound() {
        let many = format!("<a>{}</a>", "&amp;".repeat(MAX_TEXT_BYTES + 1));
        assert!(matches!(parse(many.as_bytes()), Err(Error::Bound { .. })));
    }

    #[test]
    fn unknown_and_overlong_references_are_refused() {
        assert!(matches!(parse(b"<a>&bogus;</a>"), Err(Error::Xml(_))));
        assert!(matches!(
            parse(b"<a>&#x0000000000000041;</a>"),
            Err(Error::Xml(_))
        ));
    }

    #[test]
    fn attribute_values_are_unescaped_and_prefixes_stripped() {
        let root = parse(br#"<ns0:a ns0:Ccy="E&amp;R"/>"#).expect("parses");
        assert_eq!(root.name, "a");
        assert_eq!(root.attribute("Ccy"), Some("E&R"));
    }

    #[test]
    fn names_no_writer_could_render_are_refused() {
        // The fuzzer's crash: quick-xml read an element whose name held quotes
        // and control bytes, and the rendered tree did not parse.
        let crash: &[u8] = b"<\x14'''[#\x01\x00\x00:\x00\x00\x00\x0b,\x00\x00\x00'\x00\t/>'''%=\xef\xbb\xbf==1=\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        assert!(matches!(parse(crash), Err(Error::Xml(_))));
        for bad in [
            "<a'b/>",
            "<a:/>",
            "<:a/>",
            "<1a/>",
            "<a:b:c/>",
            r#"<a b'c="1"/>"#,
        ] {
            assert!(matches!(parse(bad.as_bytes()), Err(Error::Xml(_))), "{bad}");
        }
        let long = format!("<{}/>", "a".repeat(MAX_NAME_BYTES + 1));
        assert!(matches!(parse(long.as_bytes()), Err(Error::Xml(_))));
    }

    #[test]
    fn attributes_colliding_once_prefixes_are_stripped_are_refused() {
        assert!(matches!(
            parse(br#"<a x:k="1" y:k="2"/>"#),
            Err(Error::Xml(_))
        ));
    }

    #[test]
    fn a_parsed_document_renders_and_reparses_with_one_namespace() {
        let root = parse(
            br#"<Document xmlns="urn:old" xmlns:xsi="urn:xsi"><A Ccy="EUR">1.00</A></Document>"#,
        )
        .expect("parses");
        let rendered = render(&root, "urn:new").expect("renders");
        let again = parse(rendered.as_bytes()).expect("reparses");
        assert_eq!(again.attribute("xmlns"), Some("urn:new"));
        assert_eq!(again.children[0].text, "1.00");
        assert_eq!(again.children[0].attribute("Ccy"), Some("EUR"));
    }
}
