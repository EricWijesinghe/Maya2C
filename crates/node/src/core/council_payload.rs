//! Security-council actions (Master Prompts 9 and 16): pause one module for a
//! bounded number of blocks, resume it early, retire suites (ADR-082), or
//! rotate the council's own keys (ADR-082 amendment, 2026-11-14).
//!
//! ```text
//! action   = kind nonce:u64 n:u64 (member:u8 sig[3309])*n
//! kind     = 0 module:u8 blocks:u64      pause
//!          | 1 module:u8                 resume
//!          | 2 announced_at:u64 n:u64 (suite:u8 sunset:u64 emergency:u8)*n
//!          | 3 rotation acceptance[3309]*n   rotate (n = the rotation's members)
//! rotation = activation:u64 threshold:u8 n:u64 key[1952]*n
//! ```
//!
//! Authority is the approvals, never the sender: any account may carry a
//! council action, and it lands only with `threshold` distinct members'
//! ML-DSA-65 signatures over [`CouncilAction::signing_message`], which binds
//! the council's current nonce so an approval cannot be replayed, and the
//! chain's genesis (ADR-036 tag) so it cannot be replayed on another chain
//! whose council shares a key (ADR-050 review finding, 2026-10-19).
//!
//! A rotation also carries one acceptance per new member: that member's
//! signature over [`Rotation::acceptance_message`]. It proves each new key has
//! a holder, so a typo or a hostile transaction builder cannot hand the
//! council to keys nobody holds (review of PR #544, 2026-11-14). The
//! acceptances are part of the kind, so the approvals sign them too.

use crate::core::codec::ByteReader;
use crate::crypto::{PUBLIC_KEY_LEN, SIGNATURE_LENGTH};
use crate::error::{NodeError, Result};
use crate::state::upgrades::SunsetSpec;

/// Domain of the council's signatures.
///
/// v2 since 2026-10-19: the message gained the chain tag. A new domain, not
/// only a longer message, so no v1 approval can be read as a v2 one.
pub const COUNCIL_DOMAIN: &[u8] = b"maya2c/security-council/v2";

/// Domain of a new member's acceptance of a rotation: never a valid approval,
/// and no approval is ever a valid acceptance.
pub const ROTATION_ACCEPT_DOMAIN: &[u8] = b"maya2c/security-council/rotation-accept/v1";

/// Most approvals one action may carry: well above any sane council.
pub const MAX_APPROVALS: usize = 32;

const PAUSE: u8 = 0;
const RESUME: u8 = 1;
const SUNSET_SUITES: u8 = 2;
const ROTATE: u8 = 3;

/// Most members a council may have: one approval each must fit in
/// [`MAX_APPROVALS`], or a strict majority of a larger council could never act.
pub const MAX_COUNCIL_MEMBERS: usize = MAX_APPROVALS;

/// Most suites one council sunset may retire: one per registered suite.
const MAX_SUNSETS: usize = crate::core::governance_payload::MAX_SUNSETS;

/// What the council does.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Retire suites, and nothing else (ADR-082). The only thing the council
    /// may schedule: no version, no feature, no height but this one.
    SunsetSuites {
        /// Height the sunsets are announced at. Must lie at least the
        /// council's compiled lead after the action executes.
        announced_at: u64,
        /// Suites retired, each at its own sunset height.
        sunsets: Vec<SunsetSpec>,
    },
    /// Replace the council's members and threshold from a later height.
    Rotate {
        /// The new set and when it takes over.
        rotation: Rotation,
        /// One per new member, in member order: its signature over
        /// [`Rotation::acceptance_message`].
        acceptances: Vec<Box<[u8; SIGNATURE_LENGTH]>>,
    },
}

/// A new council member set, and the height it takes over at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rotation {
    /// First height at which the new set is the council.
    pub activation: u64,
    /// Approvals an action needs under the new set.
    pub threshold: u8,
    /// The new members' ML-DSA-65 keys, by index.
    pub members: Vec<[u8; PUBLIC_KEY_LEN]>,
}

impl Rotation {
    /// Appends the encoding: also the form the council record stores.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.activation.to_le_bytes());
        buf.push(self.threshold);
        buf.extend_from_slice(&(self.members.len() as u64).to_le_bytes());
        for key in &self.members {
            buf.extend_from_slice(key);
        }
    }

    /// What each new member signs to accept a seat: the domain, the chain's
    /// tag and the rotation. No nonce: an acceptance is of this exact set at
    /// this exact height, and only a council quorum can land it.
    #[must_use]
    pub fn acceptance_message(&self, chain: &crate::core::ChainTag) -> Vec<u8> {
        let mut m = ROTATION_ACCEPT_DOMAIN.to_vec();
        m.extend_from_slice(chain.as_bytes());
        self.encode_into(&mut m);
        m
    }

    /// Decodes a rotation. The member set's soundness (a strict majority,
    /// distinct, valid keys) is checked where it is applied, not here.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for more than [`MAX_COUNCIL_MEMBERS`] or
    /// truncation.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        let activation = r.read_u64()?;
        let threshold = r.read_u8()?;
        let n = r.read_collection_len(PUBLIC_KEY_LEN)?;
        if n > MAX_COUNCIL_MEMBERS {
            return Err(NodeError::Decode(format!(
                "a council of {n} members exceeds {MAX_COUNCIL_MEMBERS}"
            )));
        }
        let members = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        Ok(Self {
            activation,
            threshold,
            members,
        })
    }
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
        match &self.kind {
            CouncilKind::Pause { module, blocks } => {
                buf.push(PAUSE);
                buf.push(*module);
                buf.extend_from_slice(&blocks.to_le_bytes());
            }
            CouncilKind::Resume { module } => {
                buf.push(RESUME);
                buf.push(*module);
            }
            CouncilKind::SunsetSuites {
                announced_at,
                sunsets,
            } => {
                buf.push(SUNSET_SUITES);
                buf.extend_from_slice(&announced_at.to_le_bytes());
                buf.extend_from_slice(&(sunsets.len() as u64).to_le_bytes());
                for sunset in sunsets {
                    buf.push(sunset.suite);
                    buf.extend_from_slice(&sunset.sunset_height.to_le_bytes());
                    buf.push(u8::from(sunset.emergency));
                }
            }
            CouncilKind::Rotate {
                rotation,
                acceptances,
            } => {
                buf.push(ROTATE);
                rotation.encode_into(buf);
                for sig in acceptances {
                    buf.extend_from_slice(sig.as_slice());
                }
            }
        }
        buf.extend_from_slice(&self.nonce.to_le_bytes());
    }

    /// What every approving member signs: domain, the chain's tag, then the
    /// action and nonce.
    #[must_use]
    pub fn signing_message(&self, chain: &crate::core::ChainTag) -> Vec<u8> {
        let mut m = COUNCIL_DOMAIN.to_vec();
        m.extend_from_slice(chain.as_bytes());
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
            SUNSET_SUITES => {
                let announced_at = r.read_u64()?;
                let count = r.read_collection_len(1 + 8 + 1)?;
                if count > MAX_SUNSETS {
                    return Err(NodeError::Decode(format!(
                        "a council sunset of {count} suites exceeds {MAX_SUNSETS}"
                    )));
                }
                let mut sunsets = Vec::with_capacity(count);
                for _ in 0..count {
                    sunsets.push(SunsetSpec {
                        suite: r.read_u8()?,
                        sunset_height: r.read_u64()?,
                        emergency: crate::core::governance_payload::read_flag(r)?,
                    });
                }
                CouncilKind::SunsetSuites {
                    announced_at,
                    sunsets,
                }
            }
            ROTATE => {
                let rotation = Rotation::decode(r)?;
                let acceptances = (0..rotation.members.len())
                    .map(|_| Ok(Box::new(r.read_array()?)))
                    .collect::<Result<_>>()?;
                CouncilKind::Rotate {
                    rotation,
                    acceptances,
                }
            }
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
    fn a_sunset_flag_byte_other_than_0_or_1_is_refused() {
        // Found by the governance_payload_decode fuzz target: `!= 0` let
        // byte 2 decode as `true`, a second wire form for one action.
        let action = CouncilAction {
            kind: CouncilKind::SunsetSuites {
                announced_at: 7,
                sunsets: vec![SunsetSpec {
                    suite: 0x10,
                    sunset_height: 9,
                    emergency: false,
                }],
            },
            nonce: 1,
            approvals: vec![],
        };
        let mut buf = Vec::new();
        action.encode_into(&mut buf);
        // kind(1) + announced_at(8) + count(8) + suite(1) + height(8) = flag at 26.
        assert_eq!(buf[26], 0);
        buf[26] = 2;
        assert!(CouncilAction::decode(&mut ByteReader::new(&buf)).is_err());
    }

    #[test]
    fn actions_round_trip_and_the_nonce_is_signed() {
        for kind in [
            CouncilKind::Pause {
                module: 2,
                blocks: 50,
            },
            CouncilKind::Resume { module: 2 },
            CouncilKind::Rotate {
                rotation: Rotation {
                    activation: 9,
                    threshold: 2,
                    members: vec![
                        [4; PUBLIC_KEY_LEN],
                        [5; PUBLIC_KEY_LEN],
                        [6; PUBLIC_KEY_LEN],
                    ],
                },
                acceptances: (0..3u8).map(|i| Box::new([i; SIGNATURE_LENGTH])).collect(),
            },
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
            let chain = crate::core::ChainTag::from_genesis([1; 32]);
            assert_ne!(a.signing_message(&chain), later.signing_message(&chain));
            // The same action on another chain is another message.
            let other = crate::core::ChainTag::from_genesis([2; 32]);
            assert_ne!(a.signing_message(&chain), a.signing_message(&other));
        }
    }

    #[test]
    fn a_rotation_naming_more_members_than_approvals_can_reach_is_refused() {
        let mut buf = vec![ROTATE];
        buf.extend_from_slice(&9u64.to_le_bytes());
        buf.push(17);
        buf.extend_from_slice(&((MAX_COUNCIL_MEMBERS + 1) as u64).to_le_bytes());
        buf.resize(buf.len() + (MAX_COUNCIL_MEMBERS + 1) * PUBLIC_KEY_LEN, 7);
        buf.extend_from_slice(&0u64.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
        let err = CouncilAction::decode(&mut ByteReader::new(&buf)).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn a_rotation_missing_an_acceptance_does_not_decode_and_accepting_is_not_approving() {
        let rotation = Rotation {
            activation: 9,
            threshold: 1,
            members: vec![[4; PUBLIC_KEY_LEN]],
        };
        let action = CouncilAction {
            kind: CouncilKind::Rotate {
                rotation: rotation.clone(),
                acceptances: vec![],
            },
            nonce: 0,
            approvals: vec![],
        };
        let mut buf = Vec::new();
        action.encode_into(&mut buf);
        // One member and no acceptance: the nonce and the approval count are
        // read as the acceptance, and the reader runs short.
        assert!(CouncilAction::decode(&mut ByteReader::new(&buf)).is_err());
        let chain = crate::core::ChainTag::from_genesis([1; 32]);
        assert_ne!(
            rotation.acceptance_message(&chain),
            action.signing_message(&chain)
        );
        assert!(
            rotation
                .acceptance_message(&chain)
                .starts_with(ROTATION_ACCEPT_DOMAIN)
        );
    }
}
