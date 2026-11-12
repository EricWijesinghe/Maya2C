//! Anchors: a record digest written on chain by an `AnchorAttestation`
//! transaction (`crates/node/src/core/identity_payload.rs`), whose wire form
//! is `root[32] ‖ expires_at u64 LE ‖ schema_len u8 ‖ schema`.
//!
//! The RPC returns a transaction's kind, sender and summary but not the root,
//! so the root is found in the block's raw bytes. That search alone could be
//! satisfied by any transaction carrying those bytes (a contract call's
//! data, say); it is safe here because every caller first checks the record's
//! ML-DSA-87 signature against the pinned key, and only a holder of that key
//! can produce a digest worth planting. The anchor adds the height — "this
//! existed by block H" — not authenticity.

use serde_json::Value;

use crate::chain::Chain;
use crate::{Error, Result};

/// The node's schema bound (`identity_payload::MAX_SCHEMA_CHARS`).
pub const MAX_SCHEMA_CHARS: usize = 64;
/// The RPC's label for the transaction kind.
pub const KIND: &str = "anchor_attestation";
/// Bytes between the root and the schema length: `expires_at`.
const EXPIRES_LEN: usize = 8;

/// What to anchor, or what an anchor must show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// The schema label, e.g. `maya2c/release/v1.0.0`.
    pub schema: String,
    /// The record digest.
    pub root: [u8; 32],
}

impl Anchor {
    /// An anchor under `schema`.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for an empty schema or one past the node's bound.
    pub fn new(schema: String, root: [u8; 32]) -> Result<Self> {
        if schema.is_empty() || schema.len() > MAX_SCHEMA_CHARS {
            return Err(Error::Malformed(format!(
                "schema is {} bytes, must be 1..={MAX_SCHEMA_CHARS}",
                schema.len()
            )));
        }
        Ok(Self { schema, root })
    }

    /// The `AnchorAttestation` fields to submit, for the operator.
    #[must_use]
    pub fn instructions(&self) -> String {
        format!(
            "anchor with an AnchorAttestation transaction:\n  schema: {}\n  root:   {}\n  expires_at: 0",
            self.schema,
            hex::encode(self.root)
        )
    }
}

/// Where an anchor transaction landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Block height.
    pub height: u64,
    /// Hex transaction id.
    pub txid: String,
}

/// Checks that `location` holds an anchor of `anchor` sent by `sender`.
///
/// # Errors
///
/// [`Error::Anchor`] naming the first thing that does not match.
pub fn verify(
    chain: &dyn Chain,
    anchor: &Anchor,
    sender: &[u8; 32],
    location: &Location,
) -> Result<()> {
    let block = chain.block(location.height)?;
    let tx = find_tx(&block, &location.txid)?;
    if tx["kind"].as_str() != Some(KIND) {
        return Err(Error::Anchor(format!(
            "tx {} is not an {KIND}",
            location.txid
        )));
    }
    if tx["sender"].as_str() != Some(hex::encode(sender).as_str()) {
        return Err(Error::Anchor(format!(
            "tx {} was not sent by the pinned key",
            location.txid
        )));
    }
    let quoted = format!("\"{}\"", anchor.schema);
    if !tx["summary"].as_str().is_some_and(|s| s.contains(&quoted)) {
        return Err(Error::Anchor(format!(
            "tx {} anchors another schema",
            location.txid
        )));
    }
    let raw = hex::decode(block["raw"].as_str().unwrap_or_default())
        .map_err(|e| Error::Malformed(format!("block raw: {e}")))?;
    if !payload_in(&raw, anchor) {
        return Err(Error::Anchor(format!(
            "block {} carries no root {} under {}",
            location.height,
            hex::encode(anchor.root),
            anchor.schema
        )));
    }
    Ok(())
}

fn find_tx<'a>(block: &'a Value, txid: &str) -> Result<&'a Value> {
    block["transactions"]
        .as_array()
        .and_then(|txs| txs.iter().find(|tx| tx["txid"].as_str() == Some(txid)))
        .ok_or_else(|| Error::Anchor(format!("tx {txid} is not in that block")))
}

/// Whether `raw` contains the `AnchorAttestation` payload for `anchor`.
fn payload_in(raw: &[u8], anchor: &Anchor) -> bool {
    let schema = anchor.schema.as_bytes();
    let tail = EXPIRES_LEN + 1 + schema.len();
    raw.windows(32 + tail).any(|w| {
        w[..32] == anchor.root
            && usize::from(w[32 + EXPIRES_LEN]) == schema.len()
            && &w[32 + EXPIRES_LEN + 1..] == schema
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;
    use crate::chain::fake::FakeChain;

    pub(crate) const SENDER: [u8; 32] = [7; 32];

    /// A chain whose block `height` anchors `anchor` from `sender` in tx `t1`.
    pub(crate) fn anchored(chain: &mut FakeChain, height: u64, anchor: &Anchor, sender: &[u8; 32]) {
        let mut raw = vec![0xAA; 50];
        raw.extend_from_slice(&anchor.root);
        raw.extend_from_slice(&0u64.to_le_bytes());
        raw.push(u8::try_from(anchor.schema.len()).unwrap());
        raw.extend_from_slice(anchor.schema.as_bytes());
        raw.extend_from_slice(&[0xBB; 20]);
        chain.blocks.insert(
            height,
            json!({
                "height": height,
                "header": { "id": format!("{height:064x}") },
                "transactions": [{
                    "txid": "t1",
                    "sender": hex::encode(sender),
                    "kind": KIND,
                    "summary": format!("Anchor an attestation under schema \"{}\".", anchor.schema),
                }],
                "raw": hex::encode(raw),
            }),
        );
        chain.tip = chain.tip.max(height);
    }

    fn setup() -> (FakeChain, Anchor, Location) {
        let anchor = Anchor::new("maya2c/test/v1".into(), [3; 32]).unwrap();
        let mut chain = FakeChain::default();
        anchored(&mut chain, 10, &anchor, &SENDER);
        (
            chain,
            anchor,
            Location {
                height: 10,
                txid: "t1".into(),
            },
        )
    }

    #[test]
    fn an_anchor_on_chain_verifies() {
        let (chain, anchor, at) = setup();
        verify(&chain, &anchor, &SENDER, &at).unwrap();
    }

    #[test]
    fn another_root_is_refused() {
        let (chain, mut anchor, at) = setup();
        anchor.root = [4; 32];
        assert!(matches!(
            verify(&chain, &anchor, &SENDER, &at),
            Err(Error::Anchor(_))
        ));
    }

    #[test]
    fn another_sender_is_refused() {
        let (chain, anchor, at) = setup();
        assert!(matches!(
            verify(&chain, &anchor, &[8; 32], &at),
            Err(Error::Anchor(_))
        ));
    }

    #[test]
    fn another_schema_is_refused() {
        let (chain, mut anchor, at) = setup();
        anchor.schema = "maya2c/test/v2".into();
        assert!(matches!(
            verify(&chain, &anchor, &SENDER, &at),
            Err(Error::Anchor(_))
        ));
    }

    #[test]
    fn a_missing_tx_or_block_is_refused() {
        let (chain, anchor, mut at) = setup();
        at.txid = "t2".into();
        assert!(verify(&chain, &anchor, &SENDER, &at).is_err());
        at.height = 11;
        assert!(verify(&chain, &anchor, &SENDER, &at).is_err());
    }

    #[test]
    fn schema_bounds_follow_the_node() {
        assert!(Anchor::new(String::new(), [0; 32]).is_err());
        assert!(Anchor::new("x".repeat(MAX_SCHEMA_CHARS + 1), [0; 32]).is_err());
        assert!(Anchor::new("x".repeat(MAX_SCHEMA_CHARS), [0; 32]).is_ok());
    }
}
