//! The outer erasure code: Reed–Solomon over one block of strands, via
//! `reed-solomon-simd` (O(n log n), GF(2^16), so block sizes are not capped at
//! 255 shards). A strand's payload is one shard; its `(block, index)` header
//! is what tells the decoder which shards it has.

use std::collections::BTreeMap;

use super::DnaError;
use super::strand::PAYLOAD;

/// Parity payloads for one block's data payloads.
///
/// # Errors
///
/// [`DnaError::Outer`] for a shard count the library does not support.
pub fn encode(data: &[[u8; PAYLOAD]], parity: usize) -> Result<Vec<[u8; PAYLOAD]>, DnaError> {
    let recovery = reed_solomon_simd::encode(data.len(), parity, data.iter())
        .map_err(|e| DnaError::Outer(e.to_string()))?;
    Ok(recovery
        .into_iter()
        .map(|shard| shard.try_into().expect("shards keep their size"))
        .collect())
}

/// Block `block`'s `data` payloads, from whichever of its data and parity
/// strands survived.
///
/// # Errors
///
/// [`DnaError::BlockLost`] if fewer than `data` strands survived, or
/// [`DnaError::Outer`] if the library refuses them.
pub fn decode(
    strands: &BTreeMap<(u16, u16), [u8; PAYLOAD]>,
    block: u16,
    data: usize,
    parity: usize,
) -> Result<Vec<[u8; PAYLOAD]>, DnaError> {
    let present = |range: core::ops::Range<usize>| -> Vec<(usize, [u8; PAYLOAD])> {
        range
            .filter_map(|i| {
                let index = u16::try_from(i).ok()?;
                strands.get(&(block, index)).map(|p| (i, *p))
            })
            .collect()
    };
    let originals = present(0..data);
    if originals.len() == data {
        return Ok(originals.into_iter().map(|(_, p)| p).collect());
    }
    let recovery: Vec<(usize, [u8; PAYLOAD])> = present(data..data + parity)
        .into_iter()
        .map(|(i, p)| (i - data, p))
        .collect();
    if originals.len() + recovery.len() < data {
        return Err(DnaError::BlockLost {
            block,
            have: originals.len() + recovery.len(),
            need: data,
        });
    }
    let restored = reed_solomon_simd::decode(
        data,
        parity,
        originals.iter().map(|(i, p)| (*i, p)),
        recovery.iter().map(|(i, p)| (*i, p)),
    )
    .map_err(|e| DnaError::Outer(e.to_string()))?;
    let known: BTreeMap<usize, [u8; PAYLOAD]> = originals.into_iter().collect();
    (0..data)
        .map(|i| {
            known
                .get(&i)
                .copied()
                .or_else(|| {
                    restored
                        .get(&i)
                        .map(|s| s.as_slice().try_into().expect("shard size"))
                })
                .ok_or(DnaError::BlockLost {
                    block,
                    have: 0,
                    need: data,
                })
        })
        .collect()
}
