//! Staking records proven against a state root (ADR-089).
//!
//! The staking layer is a Merkle tree over every `k:` record, and the layer's
//! root is one of the digests folded into the state root. A record is therefore
//! provable with a path to its layer root and the layer digests that fold to
//! the header's root. Nothing here trusts the prover: a wrong value, a wrong
//! layer, or a wrong order changes the folded root and is refused.

use custom_l1_node::state::merkle::{HASH_LEN, PathStep, verify_path};
use custom_l1_node::state::proof::{LAYER_ORDER, LayerDigest, StateLayer};

use crate::error::{LightClientError, Result};

/// Domain of one record's leaf. The node's `record_leaf` (`state/db.rs`) is
/// `pub(crate)`, so it is ported here. The light-client test
/// `the_ported_leaf_reproduces_the_node_staking_root` pins the port against the
/// staking-layer root the node computes.
const RECORD_LEAF_CONTEXT: &str = "maya aux record leaf v1";

/// The leaf a record contributes to its layer's Merkle tree.
#[must_use]
pub fn record_leaf(key: &[u8], value: &[u8]) -> [u8; HASH_LEN] {
    let mut hasher = blake3::Hasher::new_derive_key(RECORD_LEAF_CONTEXT);
    hasher.update(&(key.len() as u64).to_le_bytes());
    hasher.update(key);
    hasher.update(value);
    *hasher.finalize().as_bytes()
}

/// One record of the staking layer, with its path to the layer root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordProof {
    /// The record's key, such as `k:cmt:<epoch>`.
    pub key: Vec<u8>,
    /// The record's value as the prover read it.
    pub value: Vec<u8>,
    /// Path from the record's leaf to the staking layer root.
    pub path: Vec<PathStep>,
}

/// Checks that `records` are in the staking layer of `layers`, and that the
/// layers fold from `accounts_root` to `state_root`.
///
/// # Errors
///
/// [`LightClientError::RotationRejected`] if the layers are out of order, do
/// not fold to `state_root`, lack a staking layer, or a record is not under
/// the staking root.
pub fn verify_records(
    state_root: &[u8; HASH_LEN],
    accounts_root: &[u8; HASH_LEN],
    layers: &[LayerDigest],
    records: &[&RecordProof],
) -> Result<()> {
    check_order(layers)?;
    let staking = layers
        .iter()
        .find(|digest| digest.layer == StateLayer::Staking)
        .ok_or_else(|| rejected("the state has no staking layer".to_string()))?;

    let folded = layers
        .iter()
        .fold(*accounts_root, |root, digest| digest.layer.fold(&root, &digest.root));
    if folded != *state_root {
        return Err(rejected(
            "the layers do not fold to the header's state root".to_string(),
        ));
    }

    for record in records {
        if verify_path(&record_leaf(&record.key, &record.value), &record.path) != staking.root {
            return Err(rejected(format!(
                "record {} is not under the staking root",
                hex::encode(&record.key)
            )));
        }
    }
    Ok(())
}

/// Layers must appear in the node's fold order, each at most once. Order is
/// consensus: folding two layers the other way round gives another root.
fn check_order(layers: &[LayerDigest]) -> Result<()> {
    let mut expected = LAYER_ORDER.iter();
    for digest in layers {
        if !expected.by_ref().any(|layer| *layer == digest.layer) {
            return Err(rejected(format!(
                "layer {} is out of order or repeated",
                digest.layer.label()
            )));
        }
    }
    Ok(())
}

fn rejected(reason: String) -> LightClientError {
    LightClientError::RotationRejected(reason)
}
