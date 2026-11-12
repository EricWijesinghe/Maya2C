//! `explain_refusal`: a node refusal, in plain language, read from the
//! node's own error type.
//!
//! The source of truth is `crates/node/src/error.rs`: each `NodeError`
//! variant's `#[error("…")]` template is what the node prints, and its `///`
//! comment is why. This module embeds that file at compile time and parses it,
//! so no message or explanation is copied here, and a variant added to the
//! node is explainable on the next build with no edit to this server.

use serde_json::{Value, json};

const SOURCE: &str = include_str!("../../../crates/node/src/error.rs");

/// One `NodeError` variant as `error.rs` declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// The variant's name, e.g. `InvalidNonce`.
    pub name: String,
    /// Its display template, e.g. `invalid nonce for {address}: …`.
    pub template: String,
    /// Its doc comment, joined into paragraphs.
    pub doc: String,
}

/// Every variant of `NodeError`, in declaration order.
#[must_use]
pub fn variants() -> Vec<Variant> {
    let body = SOURCE
        .split_once("pub enum NodeError {")
        .map_or("", |(_, rest)| rest);
    let mut out = Vec::new();
    let (mut doc, mut template) = (Vec::<&str>::new(), String::new());
    let mut in_error = false;
    // Variants sit at four spaces; their fields at eight, and are skipped.
    for line in body.lines() {
        if line.starts_with('}') {
            break;
        }
        let Some(item) = line.strip_prefix("    ").filter(|l| !l.starts_with(' ')) else {
            if in_error {
                push_piece(&mut template, line);
            }
            continue;
        };
        if in_error || item.starts_with("#[error(") {
            push_piece(&mut template, item);
            in_error = !template.ends_with(")]");
        } else if let Some(text) = item.strip_prefix("///") {
            doc.push(text.trim());
        } else if let Some(name) = variant_name(item) {
            out.push(Variant {
                name,
                template: unquote(&template),
                doc: paragraphs(&doc),
            });
            doc.clear();
            template.clear();
        }
    }
    out
}

/// Appends one line of an `#[error(...)]` attribute. A line ending in `\`
/// is a string continuation: Rust drops the next line's leading whitespace.
fn push_piece(template: &mut String, line: &str) {
    if template.ends_with('\\') {
        template.pop();
        template.push_str(line.trim_start().trim_end());
    } else {
        template.push_str(line.trim());
    }
}

fn variant_name(item: &str) -> Option<String> {
    let end = item.find(|c: char| !c.is_ascii_alphanumeric() && c != '_')?;
    let name = &item[..end];
    (name.starts_with(|c: char| c.is_ascii_uppercase())
        && matches!(item.as_bytes()[end], b'{' | b'(' | b',' | b' '))
    .then(|| name.to_string())
}

/// `#[error("a {x}")]` (possibly split over lines) to `a {x}`.
fn unquote(attr: &str) -> String {
    let start = attr.find('"').map_or(0, |i| i + 1);
    let end = attr.rfind('"').unwrap_or(attr.len()).max(start);
    attr[start..end].replace("\\\"", "\"")
}

fn paragraphs(doc: &[&str]) -> String {
    doc.split(|l| l.is_empty())
        .map(|p| p.join(" "))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The template's literal pieces, between its `{…}` holes.
fn pieces(template: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push(&rest[..open]);
        rest = rest[open..]
            .find('}')
            .map_or("", |close| &rest[open + close + 1..]);
    }
    out.push(rest);
    out
}

/// The holes' text when `message` fits `template`, pieces in order.
fn fit(template: &str, message: &str) -> Option<Vec<String>> {
    let pieces = pieces(template);
    let mut at = message.find(pieces[0])? + pieces[0].len();
    let mut holes = Vec::new();
    for (i, piece) in pieces.iter().enumerate().skip(1) {
        let last = i == pieces.len() - 1;
        // The last piece anchors at the end, so a hole may hold the same text.
        let found = if last && !piece.is_empty() {
            message[at..].rfind(piece)?
        } else if piece.is_empty() {
            message.len() - at
        } else {
            message[at..].find(piece)?
        };
        holes.push(message[at..at + found].to_string());
        at += found + piece.len();
    }
    Some(holes)
}

fn literal_len(template: &str) -> usize {
    pieces(template).iter().map(|p| p.len()).sum()
}

/// The variant whose template fits `message` most specifically.
fn best(message: &str) -> Option<(Variant, Vec<String>)> {
    variants()
        .into_iter()
        .filter(|v| literal_len(&v.template) > 0)
        .filter_map(|v| fit(&v.template, message).map(|h| (v, h)))
        // The outermost fit wins (a wrapper before what it wraps), then the
        // most specific; a template that opens with a hole starts later.
        .min_by_key(|(v, _)| {
            (
                first_literal_at(&v.template, message),
                usize::MAX - literal_len(&v.template),
            )
        })
}

fn first_literal_at(template: &str, message: &str) -> usize {
    pieces(template)
        .into_iter()
        .find(|p| !p.is_empty())
        .and_then(|p| message.find(p))
        .unwrap_or(usize::MAX)
}

/// Explains `{"kind": "InvalidNonce"}` or `{"message": "<node refusal>"}`.
///
/// A message whose variant wraps another message (`transaction rejected: …`)
/// is explained one level deeper as its `cause`.
///
/// # Errors
///
/// Neither argument, an unknown kind, or a message no variant fits.
pub fn explain_refusal(args: &Value) -> Result<Value, String> {
    if let Some(kind) = args["kind"].as_str() {
        let v = variants()
            .into_iter()
            .find(|v| v.name == kind)
            .ok_or(format!("NodeError has no variant {kind}"))?;
        return Ok(describe(&v, &[], 0));
    }
    let message = args["message"]
        .as_str()
        .ok_or("kind or message is required")?;
    let (v, holes) = best(message).ok_or(format!(
        "no NodeError message fits {message:?}; it did not come from the node's error type"
    ))?;
    Ok(describe(&v, &holes, 0))
}

const MAX_DEPTH: u8 = 3;

fn describe(v: &Variant, holes: &[String], depth: u8) -> Value {
    let mut out = json!({
        "kind": v.name,
        "message_template": v.template,
        "explanation": v.doc,
        "source": "crates/node/src/error.rs",
    });
    if !holes.is_empty() {
        out["fields"] = json!(holes);
    }
    if let Ok(rule) = crate::explain(&v.name) {
        out["rule"] = rule["rule"].clone();
    }
    // A single `{0}` hole is often another error's whole message.
    if let [inner] = holes
        && depth < MAX_DEPTH
        && let Some((cause, h)) = best(inner).filter(|(c, _)| c.name != v.name)
    {
        out["cause"] = describe(&cause, &h, depth + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use custom_l1_node::error::NodeError;
    use primitive_types::U256;

    fn explained(e: &NodeError) -> Value {
        explain_refusal(&json!({"message": e.to_string()})).unwrap()
    }

    #[test]
    fn every_variant_is_parsed_with_a_template_and_a_doc() {
        let all = variants();
        assert!(all.len() > 50, "parsed {} variants", all.len());
        for v in &all {
            assert!(!v.template.is_empty(), "{} has no template", v.name);
            assert!(!v.doc.is_empty(), "{} has no doc", v.name);
        }
        assert!(all.iter().any(|v| v.name == "UpgradeRequired"
            && v.template.starts_with("upgrade required before height")));
    }

    #[test]
    fn a_continued_template_matches_its_message() {
        let e = NodeError::UnauditedShieldedCircuit {
            network: "mainnet".into(),
        };
        let out = explained(&e);
        assert_eq!(out["kind"], "UnauditedShieldedCircuit", "{out}");
        assert_eq!(out["fields"], json!(["mainnet"]));
    }

    #[test]
    fn nonce_refusal() {
        let e = NodeError::InvalidNonce {
            address: "ab".repeat(32),
            expected: 4,
            actual: 3,
        };
        let out = explained(&e);
        assert_eq!(out["kind"], "InvalidNonce");
        assert_eq!(out["fields"], json!(["ab".repeat(32), "4", "3"]));
        assert!(out["explanation"].as_str().unwrap().contains("replayed"));
        assert_eq!(out["rule"], "STF-1");
    }

    #[test]
    fn balance_refusal() {
        let e = NodeError::InsufficientBalance {
            address: "cd".repeat(32),
            required: U256::from(10),
            available: U256::from(9),
        };
        assert_eq!(explained(&e)["kind"], "InsufficientBalance");
    }

    #[test]
    fn fee_refusal() {
        let e = NodeError::FeeTooLow {
            required: U256::from(1000),
            offered: U256::from(1),
        };
        let out = explained(&e);
        assert_eq!(out["kind"], "FeeTooLow");
        assert!(out["explanation"].as_str().unwrap().contains("ADR-029"));
    }

    #[test]
    fn signature_refusals() {
        assert_eq!(
            explained(&NodeError::SignatureVerification)["kind"],
            "SignatureVerification"
        );
        assert_eq!(
            explained(&NodeError::HashSignatureVerification)["kind"],
            "HashSignatureVerification"
        );
        assert_eq!(
            explained(&NodeError::MissingSignature)["kind"],
            "MissingSignature"
        );
    }

    #[test]
    fn encoding_refusal_behind_the_rpc_prefix() {
        let msg = format!(
            "malformed transaction: {}",
            NodeError::Decode("trailing bytes".into())
        );
        let out = explain_refusal(&json!({"message": msg})).unwrap();
        assert_eq!(out["kind"], "Decode");
        assert_eq!(out["fields"], json!(["trailing bytes"]));
    }

    #[test]
    fn admission_refusal() {
        let e = NodeError::EmptySender {
            address: "ee".repeat(32),
        };
        assert_eq!(explained(&e)["kind"], "EmptySender");
    }

    #[test]
    fn protocol_upgrade_refusal() {
        let e = NodeError::UpgradeRequired {
            version: 3,
            height: 900,
            supported: 2,
        };
        assert_eq!(explained(&e)["kind"], "UpgradeRequired");
    }

    #[test]
    fn block_refusal() {
        let e = NodeError::StateRootMismatch {
            expected: "11".repeat(32),
            actual: "22".repeat(32),
        };
        assert_eq!(explained(&e)["kind"], "StateRootMismatch");
    }

    #[test]
    fn a_wrapped_refusal_names_its_cause() {
        let inner = NodeError::InvalidNonce {
            address: "ab".repeat(32),
            expected: 1,
            actual: 0,
        };
        let e = NodeError::MempoolRejected(inner.to_string());
        let out = explained(&e);
        assert_eq!(out["kind"], "MempoolRejected");
        assert_eq!(out["cause"]["kind"], "InvalidNonce");
    }

    #[test]
    fn by_kind_and_unknowns() {
        let out = explain_refusal(&json!({"kind": "BlockTooLarge"})).unwrap();
        assert_eq!(out["kind"], "BlockTooLarge");
        assert!(explain_refusal(&json!({"kind": "NoSuchThing"})).is_err());
        assert!(explain_refusal(&json!({"message": "the cat sat on the mat"})).is_err());
        assert!(explain_refusal(&json!({})).is_err());
    }
}
