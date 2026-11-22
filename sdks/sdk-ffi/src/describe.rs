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
use custom_l1_node::core::{ChainTag, Transaction, TxKind};

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
    /// The account a wire-v13 frame names as its fee payer, hex; `None`
    /// otherwise. A claim unless `sponsor_verified` is `true`.
    pub sponsor: Option<String>,
    /// Whether the sponsor's signature holds over the signed frame on the
    /// chain asked about. `summary` and `details` never mention the sponsor:
    /// a valid sponsorship can still be stripped after signing. A binding
    /// shows a verified `sponsor` as an offer to pay, an unverified one as a
    /// claim, and neither as the payer.
    pub sponsor_verified: bool,
}

fn record(
    kind: &TxKind,
    d: Description,
    sponsor: Option<String>,
    sponsor_verified: bool,
) -> TxDescription {
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
        sponsor_verified,
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
    Ok(record(&kind, describe_kind(&kind), None, false))
}

/// Describes a whole wire frame (any version the node decodes, v13 included)
/// as it would apply on the chain whose genesis id is `genesis_id`. A
/// sponsor is checked against that chain (`Transaction::sponsor_holds`) and
/// reported in `sponsor_verified`; the words never state it as the payer.
///
/// # Errors
///
/// [`SdkError::Malformed`] when the node's decoder refuses the frame, or
/// `genesis_id` is not 32 bytes.
#[uniffi::export]
pub fn describe_transaction(
    frame: Vec<u8>,
    genesis_id: Vec<u8>,
) -> Result<TxDescription, SdkError> {
    let genesis = <[u8; 32]>::try_from(genesis_id).map_err(|_| SdkError::Malformed {
        what: "genesis id".to_string(),
    })?;
    let tx = Transaction::from_bytes(&frame).map_err(|_| SdkError::Malformed {
        what: "transaction".to_string(),
    })?;
    let verified = tx.sponsor_holds(&ChainTag::from_genesis(genesis));
    // Never stated in the words: even a valid sponsorship can be stripped
    // after the sender signs, so it is an offer, carried in `sponsor`.
    let described = describe(&tx);
    let sponsor = tx.sponsor.as_ref().map(|s| hex::encode(s.address()));
    Ok(record(&tx.kind, described, sponsor, verified))
}
