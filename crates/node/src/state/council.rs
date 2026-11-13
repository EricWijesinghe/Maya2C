//! The security council's record and its one power: a bounded pause.
//!
//! A council pause is a breaker record (`invariant_guard::breaker`) written by
//! a quorum of named keys instead of by an invariant check. That choice bounds
//! what the council can do to exactly what the automatic breaker can:
//!
//! - it halts one **module** — never plain transfers, staking, or the council
//!   itself — so the chain keeps finalizing and value keeps moving;
//! - it expires on its own after at most `max_pause_blocks`; extending it
//!   takes another quorum, and every action advances the nonce;
//! - it is recorded under the governance prefix, so it is in the state root
//!   and every node enforces it identically.
//!
//! Present only when genesis names a council; otherwise council actions are
//! refused and no record exists.

use crate::core::codec::ByteReader;
use crate::core::council_payload::{CouncilAction, CouncilKind};
use crate::crypto::PUBLIC_KEY_LEN;
use crate::crypto::keys::VerifyingKey;
use crate::error::{NodeError, Result};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::invariant_guard::Module;
use crate::state::invariant_guard::breaker::{BreakerRecord, Invariant, guard_key};
use crate::state::upgrades::{
    COUNCIL_SUNSET_LEAD_BLOCKS, EffectiveSchedule, ScheduleRequest, SunsetSpec,
};
use crate::upgrade::FeatureSet;

/// The council record, under the governance prefix.
pub const COUNCIL_KEY: &[u8] = b"g:council";

/// The council as the state holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CouncilRecord {
    /// Approvals an action needs.
    pub threshold: u8,
    /// Longest pause one action may impose.
    pub max_pause_blocks: u64,
    /// The next action's nonce.
    pub nonce: u64,
    /// Members' ML-DSA-65 keys, by index.
    pub members: Vec<[u8; PUBLIC_KEY_LEN]>,
}

impl CouncilRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![1u8, self.threshold];
        out.extend_from_slice(&self.max_pause_blocks.to_le_bytes());
        out.extend_from_slice(&self.nonce.to_le_bytes());
        out.extend_from_slice(&(self.members.len() as u64).to_le_bytes());
        for m in &self.members {
            out.extend_from_slice(m);
        }
        out
    }

    /// Decodes a record.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong version or malformed bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != 1 {
            return Err(NodeError::Decode("council record version".to_string()));
        }
        let threshold = r.read_u8()?;
        let max_pause_blocks = r.read_u64()?;
        let nonce = r.read_u64()?;
        let n = r.read_collection_len(PUBLIC_KEY_LEN)?;
        let members = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        r.finish()?;
        Ok(Self {
            threshold,
            max_pause_blocks,
            nonce,
            members,
        })
    }
}

impl StateDB {
    /// The committed council, for RPC and operators.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub fn committed_council(&self) -> Result<Option<CouncilRecord>> {
        self.raw_get(COUNCIL_KEY)?
            .map(|b| CouncilRecord::decode(&b))
            .transpose()
    }

    /// Executes a council action.
    ///
    /// # Errors
    ///
    /// No council, a stale nonce, too few valid distinct approvals, an
    /// unknown module, or a pause longer than the council may impose.
    pub(crate) fn apply_council(
        &self,
        overlay: &mut Overlay,
        action: &CouncilAction,
        context: BlockContext,
    ) -> Result<()> {
        let refuse =
            |why: String| Err(NodeError::Network(format!("council action refused: {why}")));
        let Some(bytes) = self.record(overlay, COUNCIL_KEY)? else {
            return refuse("this chain has no security council".into());
        };
        let mut council = CouncilRecord::decode(&bytes)?;
        if action.nonce != council.nonce {
            return refuse(format!("nonce {} is not {}", action.nonce, council.nonce));
        }
        let message = action.signing_message(self.get_chain_tag()?);
        let mut approved = std::collections::BTreeSet::new();
        for (member, sig) in &action.approvals {
            let Some(key) = council.members.get(usize::from(*member)) else {
                continue;
            };
            if VerifyingKey::from_bytes(key)?.verify(&message, sig).is_ok() {
                approved.insert(*member);
            }
        }
        if approved.len() < usize::from(council.threshold) {
            return refuse(format!(
                "{} valid approvals, {} needed",
                approved.len(),
                council.threshold
            ));
        }
        let module_tag = match &action.kind {
            CouncilKind::Pause { module, .. } | CouncilKind::Resume { module } => *module,
            CouncilKind::SunsetSuites {
                announced_at,
                sunsets,
            } => {
                return self.stage_council_sunset(
                    overlay,
                    &mut council,
                    *announced_at,
                    sunsets,
                    context,
                );
            }
        };
        let module = Module::from_tag(module_tag)
            .ok_or_else(|| NodeError::Decode(format!("unknown module {module_tag}")))?;
        // Tags 13 and 14 (ADR-135) are unknown to a pre-v4 binary, which
        // refuses them at the line above; refusing them here until their bit
        // activates keeps both binaries agreeing on every earlier block.
        let dark = match module {
            Module::Nft => !context.nft_active(),
            Module::Names => !context.names_active(),
            // Listed, not `_`: a new module must decide its own gate here.
            Module::Channels
            | Module::Dex
            | Module::Shielded
            | Module::Oracle
            | Module::Governance
            | Module::Vm
            | Module::Sealed
            | Module::Identity
            | Module::Rwa
            | Module::Htlc
            | Module::ThreatIntel
            | Module::Iot
            | Module::Vault => false,
        };
        if dark {
            return Err(NodeError::Decode(format!("unknown module {module_tag}")));
        }
        // Tag 13 (ADR-135 P-7) is unknown to a pre-v4 binary, which refuses
        // it at the line above; refusing it here until bit 46 activates keeps
        // both binaries agreeing on every earlier block.
        if module == Module::Scheduled && !context.scheduled_active() {
            return Err(NodeError::Decode(format!("unknown module {module_tag}")));
        }
        let record = match action.kind {
            // Returned above; refused here rather than trusted to stay that way.
            CouncilKind::SunsetSuites { .. } => return refuse("a sunset is not a pause".into()),
            CouncilKind::Pause { blocks, .. } => {
                if blocks == 0 || blocks > council.max_pause_blocks {
                    return refuse(format!(
                        "a pause of {blocks} blocks; at most {}",
                        council.max_pause_blocks
                    ));
                }
                BreakerRecord {
                    module,
                    invariant: Invariant::CouncilPause,
                    tripped_at: context.height,
                    until: context.height.saturating_add(blocks),
                }
            }
            // Ending a pause writes one that has already expired, so the
            // record still says who last acted and when.
            CouncilKind::Resume { .. } => BreakerRecord {
                module,
                invariant: Invariant::CouncilPause,
                tripped_at: context.height,
                until: context.height,
            },
        };
        StateDB::put_record(overlay, guard_key(module), record.encode().to_vec());
        council.nonce = council.nonce.saturating_add(1);
        StateDB::put_record(overlay, COUNCIL_KEY.to_vec(), council.encode());
        Ok(())
    }

    /// Schedules a suite sunset the council proposed (ADR-082). Only a sunset:
    /// the request carries no version and no feature, and it must be announced
    /// at least the council's compiled lead after this action executes.
    fn stage_council_sunset(
        &self,
        overlay: &mut Overlay,
        council: &mut CouncilRecord,
        announced_at: u64,
        sunsets: &[SunsetSpec],
        context: BlockContext,
    ) -> Result<()> {
        let request = ScheduleRequest {
            activation: announced_at,
            version: None,
            features: FeatureSet::default(),
            sunsets: sunsets.to_vec(),
        };
        // No emergency window on this path (H3): the migration window is the
        // only exit clock, and it is the one the derived lead already covers.
        if sunsets.iter().any(|s| s.emergency) {
            return Err(NodeError::Network(
                "council action refused: a council sunset has no emergency window".into(),
            ));
        }
        let default = self.default_after(overlay, &[])?;
        let admitted = self.effective_schedule_through(overlay)?.admit(
            &request,
            context.height,
            COUNCIL_SUNSET_LEAD_BLOCKS,
            default,
        );
        if let Err(error) = admitted {
            return Err(NodeError::Network(format!(
                "council action refused: {error}"
            )));
        }
        let (entry, records) = EffectiveSchedule::entry_for(&request);
        StateDB::stage_schedule(overlay, entry, &records);
        council.nonce = council.nonce.saturating_add(1);
        StateDB::put_record(overlay, COUNCIL_KEY.to_vec(), council.encode());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn the_record_round_trips() {
        let r = CouncilRecord {
            threshold: 2,
            max_pause_blocks: 600,
            nonce: 3,
            members: vec![
                [1; PUBLIC_KEY_LEN],
                [2; PUBLIC_KEY_LEN],
                [3; PUBLIC_KEY_LEN],
            ],
        };
        assert_eq!(CouncilRecord::decode(&r.encode()).unwrap(), r);
    }
}
