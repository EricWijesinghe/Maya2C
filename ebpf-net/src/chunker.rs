//! A block body into relay datagrams.

use maya_ebpf_net_common::header::RelayHeader;

use crate::error::RelayError;
use crate::seal::RelayKey;

/// Seals the chunks of one block under one key.
///
/// Indexed rather than iterated, so a sender reuses one buffer for every
/// datagram and can resend a single chunk without re-sealing the rest.
#[derive(Debug)]
pub struct BlockSealer<'a> {
    key: &'a RelayKey,
    block_id: [u8; 32],
    body: &'a [u8],
    body_len: u32,
    chunk_count: u16,
}

impl<'a> BlockSealer<'a> {
    /// Prepares `body` — the block's `to_bytes` — for relay under `key`.
    ///
    /// # Errors
    ///
    /// [`RelayError::BodyTooLarge`] or [`RelayError::Header`] for a body no
    /// header can describe.
    pub fn new(key: &'a RelayKey, block_id: [u8; 32], body: &'a [u8]) -> Result<Self, RelayError> {
        let body_len = u32::try_from(body.len()).map_err(|_| RelayError::BodyTooLarge(body.len()))?;
        let first = RelayHeader::new(key.id(), block_id, 0, body_len)?;
        Ok(Self {
            key,
            block_id,
            body,
            body_len,
            chunk_count: first.chunk_count(),
        })
    }

    /// Datagrams the block needs.
    #[must_use]
    pub const fn chunk_count(&self) -> u16 {
        self.chunk_count
    }

    /// Writes datagram `index` into `out`, replacing its contents.
    ///
    /// # Errors
    ///
    /// [`RelayError::Header`] for an index at or past [`Self::chunk_count`].
    pub fn seal(&self, index: u16, out: &mut Vec<u8>) -> Result<(), RelayError> {
        let header = RelayHeader::new(self.key.id(), self.block_id, index, self.body_len)?;
        let start = header.body_offset();
        let end = start + header.plaintext_len();
        let plaintext = self
            .body
            .get(start..end)
            .ok_or(RelayError::PlaintextLength {
                actual: self.body.len().saturating_sub(start),
                expected: header.plaintext_len(),
            })?;
        self.key.seal(&header, plaintext, out)
    }
}

#[cfg(test)]
mod tests {
    use maya_ebpf_net_common::header::{CHUNK_LEN, MAX_BODY_LEN};

    use super::*;

    #[test]
    fn a_body_is_cut_into_the_implied_number_of_exactly_sized_datagrams() {
        let key = RelayKey::from_bytes(&[4; 32]);
        let body = vec![0x5A; CHUNK_LEN * 2 + 1];
        let sealer = BlockSealer::new(&key, [1; 32], &body).unwrap();
        assert_eq!(sealer.chunk_count(), 3);
        let mut out = Vec::new();
        let lengths: Vec<usize> = (0..3)
            .map(|i| {
                sealer.seal(i, &mut out).unwrap();
                out.len()
            })
            .collect();
        assert_eq!(lengths, [1232, 1232, 73]);
        assert!(sealer.seal(3, &mut out).is_err());
    }

    #[test]
    fn empty_and_oversized_bodies_are_refused() {
        let key = RelayKey::from_bytes(&[4; 32]);
        assert!(BlockSealer::new(&key, [1; 32], &[]).is_err());
        let huge = vec![0u8; MAX_BODY_LEN as usize + 1];
        assert!(BlockSealer::new(&key, [1; 32], &huge).is_err());
    }
}
