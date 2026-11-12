//! The public consensus data one source serves: its newest quorum-attested
//! checkpoint (ADR-038) and the block it serves at that height.
//!
//! Signatures are carried as committee indices only. Checking them needs the
//! committee's public keys, which the gateway does not serve (`get_validators`
//! is not on its allowlist), so presence is all a third party can count.

use maya_sentinel::obs::Transport;
use serde_json::Value;

/// A checkpoint as `get_checkpoint` serves it, signatures reduced to who signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    /// Staking epoch of the attesting committee.
    pub epoch: u64,
    /// Height of the attested block.
    pub height: u64,
    /// Hex id of the attested block.
    pub block: String,
    /// Committee indices that signed, in the order served.
    pub signers: Vec<u16>,
}

/// The block a source serves at a finalized height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalBlock {
    /// Hex header id.
    pub id: String,
    /// Wire size in bytes (the `raw` hex, halved).
    pub bytes: u64,
}

/// What one poll of one source saw beyond the sentinel's observation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublicObs {
    /// Newest checkpoint, `None` when the source holds none.
    pub checkpoint: Option<Checkpoint>,
    /// The block at the checkpoint's height, when the source served it.
    pub block: Option<FinalBlock>,
    /// Why that block could not be read, so a skipped check is visible.
    pub block_error: Option<String>,
}

/// Parses a `get_checkpoint` answer. `null` and any malformed shape are `None`.
#[must_use]
pub fn parse_checkpoint(v: &Value) -> Option<Checkpoint> {
    let signers = v
        .get("signatures")?
        .as_array()?
        .iter()
        .map(|pair| u16::try_from(pair.get(0)?.as_u64()?).ok())
        .collect::<Option<Vec<u16>>>()?;
    Some(Checkpoint {
        epoch: v.get("epoch")?.as_u64()?,
        height: v.get("height")?.as_u64()?,
        block: v.get("block")?.as_str()?.to_owned(),
        signers,
    })
}

fn parse_block(v: &Value) -> Option<FinalBlock> {
    let raw = v.get("raw")?.as_str()?;
    Some(FinalBlock {
        id: v.pointer("/header/id")?.as_str()?.to_owned(),
        bytes: u64::try_from(raw.len() / 2).ok()?,
    })
}

/// Reads one source's newest checkpoint and the block it serves there.
///
/// # Errors
///
/// The transport's error text when `get_checkpoint` cannot be called. A
/// source that answers `null`, or cannot serve the block, is not an error:
/// those fields stay `None`.
pub async fn observe_public<T: Transport>(t: &T) -> Result<PublicObs, String> {
    let cp = t
        .call("get_checkpoint", vec![])
        .await
        .map_err(|e| format!("get_checkpoint failed: {e}"))?;
    let Some(checkpoint) = parse_checkpoint(&cp) else {
        return Ok(PublicObs::default());
    };
    let (block, block_error) = match t.call("get_block_by_height", vec![checkpoint.height]).await {
        Ok(v) => match parse_block(&v) {
            Some(b) => (Some(b), None),
            None => (None, Some("answered without header.id or raw".to_owned())),
        },
        Err(e) => (None, Some(e)),
    };
    Ok(PublicObs {
        checkpoint: Some(checkpoint),
        block,
        block_error,
    })
}

/// Polls every source once, in order.
pub async fn gather_public<T: Transport>(
    nodes: &[(String, T)],
) -> Vec<(String, Result<PublicObs, String>)> {
    let mut out = Vec::with_capacity(nodes.len());
    for (name, t) in nodes {
        out.push((name.clone(), observe_public(t).await));
    }
    out
}
