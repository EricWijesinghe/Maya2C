//! Clear-sign words for any payload or frame, through the node's own codec
//! and `core::describe` (ADR-135 I7).
//!
//! Python, Kotlin and Swift get the same sentence the desktop wallet, the
//! explorer and the CLI show, for every transaction kind the node knows,
//! including the v4 forms and wire-v13 sponsored frames. Nothing is reworded
//! here: a binding that phrased a kind its own way would be the blind spot an
//! attacker aims for.

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::describe::{Description, Review, describe, describe_kind};
use custom_l1_node::core::{Transaction, TxKind};

use crate::SdkError;

/// One labelled value: an address, id or exact amount, in full.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Detail {
    /// What the value is, e.g. `pay to` or `asset`.
    pub label: String,
    /// The value in full; addresses and ids are hex.
    pub value: String,
}

/// What a payload or a transaction does, in the node's words.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TxDescription {
    /// The kind's name, e.g. `mint_asset`.
    pub kind: String,
    /// One sentence.
    pub summary: String,
    /// Whether a signer should look twice before signing.
    pub careful: bool,
    /// The values the sentence refers to, in full and in payload order.
    pub details: Vec<Detail>,
    /// The fee payer of a wire-v13 sponsored frame, hex; `None` otherwise.
    pub sponsor: Option<String>,
}

fn record(kind: &TxKind, d: Description, sponsor: Option<String>) -> TxDescription {
    TxDescription {
        kind: kind.label().to_owned(),
        summary: d.summary,
        careful: d.review == Review::Careful,
        details: d
            .details
            .into_iter()
            .map(|(label, value)| Detail {
                label: label.to_owned(),
                value,
            })
            .collect(),
        sponsor,
    }
}

/// Describes a bare payload (`tag || fields`), e.g. a v4 token, NFT, name or
/// schedule form.
///
/// # Errors
///
/// [`SdkError::Malformed`] when the node's decoder refuses the bytes or they
/// carry trailing data.
#[uniffi::export]
pub fn describe_payload(payload: Vec<u8>) -> Result<TxDescription, SdkError> {
    let malformed = |_| SdkError::Malformed {
        what: "payload".to_string(),
    };
    let mut reader = ByteReader::new(&payload);
    let kind = TxKind::decode(&mut reader).map_err(malformed)?;
    reader.finish().map_err(malformed)?;
    Ok(record(&kind, describe_kind(&kind), None))
}

/// Describes a whole wire frame (any version the node decodes, v13 included).
///
/// # Errors
///
/// [`SdkError::Malformed`] when the node's decoder refuses the frame.
#[uniffi::export]
pub fn describe_transaction(frame: Vec<u8>) -> Result<TxDescription, SdkError> {
    let tx = Transaction::from_bytes(&frame).map_err(|_| SdkError::Malformed {
        what: "transaction".to_string(),
    })?;
    let sponsor = tx.sponsor.as_ref().map(|s| hex::encode(s.address()));
    Ok(record(&tx.kind, describe(&tx), sponsor))
}
