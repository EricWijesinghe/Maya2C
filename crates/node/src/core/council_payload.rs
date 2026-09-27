//! Security-council actions (Master Prompts 9 and 16): pause one module for a
//! bounded number of blocks, or resume it early.
//!
//! ```text
//! action   = kind nonce:u64 n:u64 (member:u8 sig[3309])*n
//! kind     = 0 module:u8 blocks:u64      pause
//!          | 1 module:u8                 resume
//! ```
//!
//! Authority is the approvals, never the sender: any account may carry a
//! council action, and it lands only with `threshold` distinct members'
//! ML-DSA-65 signatures over [`CouncilAction::signing_message`], which binds
//! the council's current nonce so an approval cannot be replayed.

use crate::core::codec::ByteReader;
use crate::crypto::SIGNATURE_LENGTH;
use crate::error::{NodeError, Result};

/// Domain of the council's signatures.
pub const COUNCIL_DOMAIN: &[u8] = b"maya2c/security-council/v1";

/// Most approvals one action may carry: well above any sane council.
pub const MAX_APPROVALS: usize = 32;

const PAUSE: u8 = 0;
const RESUME: u8 = 1;

/// What the council does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CouncilKind {
    /// Halt `module` (a `invariant_guard::Module` tag) for `blocks` blocks.
    Pause {
        /// Module tag.
        module: u8,
        /// Length of the pause, capped by the council's `max_pause_blocks`.
        blocks: u64,
    },
    /// End a pause early.
    Resume {
        /// Module tag.
        module: u8,
    },
}

/// One council action and its approvals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CouncilAction {
    /// The action.
    pub kind: CouncilKind,
    /// Must equal the council record's nonce; it advances on every action.
    pub nonce: u64,
    /// `(member index, signature)`, distinct members.
    pub approvals: Vec<(u8, Box<[u8; SIGNATURE_LENGTH]>)>,
}

impl CouncilAction {
    fn encode_kind(&self, buf: &mut Vec<u8>) {
        match self.kind {
            CouncilKind::Pause { module, blocks } => {
                buf.push(PAUSE);
                buf.push(module);
                buf.extend_from_slice(&blocks.to_le_bytes());
            }
            CouncilKind::Resume { module } => {
                buf.push(RESUME);
                buf.push(module);
            }
        }
        buf.extend_from_slice(&self.nonce.to_le_bytes());
    }

    /// What every approving member signs.
    #[must_use]
    pub fn signing_message(&self) -> Vec<u8> {
        let mut m = COUNCIL_DOMAIN.to_vec();
        self.encode_kind(&mut m);
        m
    }

    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        self.encode_kind(buf);
        buf.extend_from_slice(&(self.approvals.len() as u64).to_le_bytes());
        for (member, sig) in &self.approvals {
            buf.push(*member);
            buf.extend_from_slice(sig.as_slice());
        }
    }

    /// Decodes one action.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for an unknown kind, more than
    /// [`MAX_APPROVALS`], or truncation.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        let kind = match r.read_u8()? {
            PAUSE => CouncilKind::Pause {
                module: r.read_u8()?,
                blocks: r.read_u64()?,
            },
            RESUME => CouncilKind::Resume {
                module: r.read_u8()?,
            },
            t => return Err(NodeError::Decode(format!("council action kind {t}"))),
        };
        let nonce = r.read_u64()?;
        let n = r.read_collection_len(1 + SIGNATURE_LENGTH)?;
        if n > MAX_APPROVALS {
            return Err(NodeError::Decode(format!("{n} council approvals")));
        }
        let approvals = (0..n)
            .map(|_| Ok((r.read_u8()?, Box::new(r.read_array()?))))
            .collect::<Result<_>>()?;
        Ok(Self {
            kind,
            nonce,
            approvals,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn actions_round_trip_and_the_nonce_is_signed() {
        for kind in [
            CouncilKind::Pause {
                module: 2,
                blocks: 50,
            },
            CouncilKind::Resume { module: 2 },
        ] {
            let a = CouncilAction {
                kind,
                nonce: 7,
                approvals: vec![
                    (0, Box::new([1; SIGNATURE_LENGTH])),
                    (2, Box::new([2; SIGNATURE_LENGTH])),
                ],
            };
            let mut buf = Vec::new();
            a.encode_into(&mut buf);
            let mut r = ByteReader::new(&buf);
            assert_eq!(CouncilAction::decode(&mut r).unwrap(), a);
            r.finish().unwrap();
            let mut later = a.clone();
            later.nonce = 8;
            assert_ne!(a.signing_message(), later.signing_message());
        }
    }
}
