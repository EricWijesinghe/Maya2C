//! The security council's record and its powers: a bounded pause, a suite
//! sunset under a compiled lead (ADR-082), and the rotation of its own keys.
//!
//! A council pause is a breaker record (`invariant_guard::breaker`) written by
//! a quorum of named keys instead of by an invariant check. That choice bounds
//! what the council can do to exactly what the automatic breaker can:
//!
//! - it halts one **module** — never plain transfers, staking, Governance, or
//!   the council itself — so the chain keeps finalizing, value keeps moving,
//!   and the vote that would answer the council stays open (founder audit Q7);
//! - it expires on its own after at most `max_pause_blocks`, and the council
//!   cannot renew it until `COUNCIL_RENEWAL_COOLDOWN_BLOCKS` after it ends.
//!   The end is kept in the council record (`last_pause_end`), not read back
//!   from the breaker record, because an automatic trip overwrites that one;
//!   every action advances the nonce;
//! - it is recorded under the governance prefix, so it is in the state root
//!   and every node enforces it identically.
//!
//! A rotation (founder audit Q8) needs a quorum of the council in force and an
//! acceptance signed by every new member. It waits in the record, public, for
//! at least `COUNCIL_ROTATION_LEAD_BLOCKS`; [`CouncilRecord::at`] makes it the
//! council from its activation height (ADR-082 amendment, 2026-11-14). It
//! cannot recover a council that has lost its quorum: that still takes a
//! re-genesis.
//!
//! Present only when genesis names a council; otherwise council actions are
//! refused and no record exists.

use std::collections::BTreeMap;

use crate::core::ChainTag;
use crate::core::codec::ByteReader;
use crate::core::council_payload::{CouncilAction, CouncilKind, MAX_COUNCIL_MEMBERS, Rotation};
use crate::core::governance_payload::read_flag;
use crate::crypto::keys::VerifyingKey;
use crate::crypto::{PUBLIC_KEY_LEN, SIGNATURE_LENGTH};
use crate::error::{NodeError, Result};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::invariant_guard::Module;
use crate::state::invariant_guard::breaker::{BreakerRecord, Invariant, guard_key};
use crate::state::upgrades::{
    COUNCIL_SUNSET_LEAD_BLOCKS, EffectiveSchedule, ScheduleRequest, SunsetSpec,
};
use crate::upgrade::FeatureSet;
use maya_governance::limits::{
    COUNCIL_RENEWAL_COOLDOWN_BLOCKS, COUNCIL_ROTATION_LEAD_BLOCKS, MAX_SCHEDULE_HORIZON_BLOCKS,
};

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
    /// A rotation the council scheduled and that has not yet taken over.
    pub pending: Option<Rotation>,
    /// Height of the council's last suite sunset, for the renewal cooldown.
    pub last_sunset_at: Option<u64>,
    /// End of the council's last pause of each module (by module tag), for
    /// the renewal cooldown; a resume sets it to the resume's height.
    pub last_pause_end: BTreeMap<u8, u64>,
}

impl CouncilRecord {
    /// Encodes the record.
    ///
    /// Version 1 when none of the later fields is set, so a council that never
    /// acted keeps the bytes (and the genesis root) it always had; version 2
    /// otherwise. One form per value: a version 2 record with every later
    /// field empty does not decode.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let extended = self.pending.is_some()
            || self.last_sunset_at.is_some()
            || !self.last_pause_end.is_empty();
        let mut out = vec![if extended { 2u8 } else { 1 }, self.threshold];
        out.extend_from_slice(&self.max_pause_blocks.to_le_bytes());
        out.extend_from_slice(&self.nonce.to_le_bytes());
        out.extend_from_slice(&(self.members.len() as u64).to_le_bytes());
        for m in &self.members {
            out.extend_from_slice(m);
        }
        if extended {
            out.push(u8::from(self.last_sunset_at.is_some()));
            out.extend_from_slice(&self.last_sunset_at.unwrap_or(0).to_le_bytes());
            out.push(u8::from(self.pending.is_some()));
            if let Some(rotation) = &self.pending {
                rotation.encode_into(&mut out);
            }
            out.extend_from_slice(&(self.last_pause_end.len() as u64).to_le_bytes());
            for (tag, end) in &self.last_pause_end {
                out.push(*tag);
                out.extend_from_slice(&end.to_le_bytes());
            }
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
        let version = r.read_u8()?;
        if version != 1 && version != 2 {
            return Err(NodeError::Decode("council record version".to_string()));
        }
        let threshold = r.read_u8()?;
        let max_pause_blocks = r.read_u64()?;
        let nonce = r.read_u64()?;
        let n = r.read_collection_len(PUBLIC_KEY_LEN)?;
        let members = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        let (mut last_sunset_at, mut pending) = (None, None);
        let mut last_pause_end = BTreeMap::new();
        if version == 2 {
            let has_sunset = read_flag(&mut r)?;
            let height = r.read_u64()?;
            if has_sunset {
                last_sunset_at = Some(height);
            } else if height != 0 {
                return Err(NodeError::Decode(
                    "council record: stray sunset height".into(),
                ));
            }
            if read_flag(&mut r)? {
                pending = Some(Rotation::decode(&mut r)?);
            }
            for _ in 0..r.read_collection_len(1 + 8)? {
                let tag = r.read_u8()?;
                // Ascending and known: one form per record.
                if Module::from_tag(tag).is_none()
                    || last_pause_end
                        .last_key_value()
                        .is_some_and(|(t, _)| *t >= tag)
                {
                    return Err(NodeError::Decode(format!(
                        "council record: pause end for module {tag} out of order or unknown"
                    )));
                }
                last_pause_end.insert(tag, r.read_u64()?);
            }
            if last_sunset_at.is_none() && pending.is_none() && last_pause_end.is_empty() {
                return Err(NodeError::Decode("council record: empty version 2".into()));
            }
        }
        r.finish()?;
        Ok(Self {
            threshold,
            max_pause_blocks,
            nonce,
            members,
            pending,
            last_sunset_at,
            last_pause_end,
        })
    }

    /// The council in force at `height`: a pending rotation whose activation
    /// has come replaces the members and threshold. A pure function of the
    /// record and the height, so the handover needs no per-block hook and
    /// every node agrees on who the council is at every height.
    #[must_use]
    pub fn at(mut self, height: u64) -> Self {
        if let Some(rotation) = self.pending.take_if(|r| r.activation <= height) {
            self.threshold = rotation.threshold;
            self.members = rotation.members;
        }
        self
    }
}

/// Whether `members` under `threshold` is a set genesis or a rotation may
/// name: 1 to [`MAX_COUNCIL_MEMBERS`] distinct valid keys and a strict
/// majority (a minority could otherwise act over the majority's head).
///
/// # Errors
///
/// Why the set is refused.
pub fn check_member_set(
    threshold: u8,
    members: &[[u8; PUBLIC_KEY_LEN]],
) -> std::result::Result<(), String> {
    let n = members.len();
    if n == 0
        || n > MAX_COUNCIL_MEMBERS
        || usize::from(threshold) * 2 <= n
        || usize::from(threshold) > n
    {
        return Err(format!(
            "threshold {threshold} of {n} is not a strict majority of 1 to {MAX_COUNCIL_MEMBERS} members"
        ));
    }
    for (i, key) in members.iter().enumerate() {
        VerifyingKey::from_bytes(key).map_err(|e| format!("members[{i}]: {e}"))?;
        if members[..i].contains(key) {
            return Err(format!("members[{i}] repeats"));
        }
    }
    Ok(())
}

impl StateDB {
    /// The committed council as it is in force at `height`, for RPC and
    /// operators: a rotation whose activation has come is applied, although
    /// the stored record holds it as pending until the council's next action
    /// writes the record back.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub fn committed_council(&self, height: u64) -> Result<Option<CouncilRecord>> {
        self.raw_get(COUNCIL_KEY)?
            .map(|b| CouncilRecord::decode(&b).map(|c| c.at(height)))
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
        let mut council = CouncilRecord::decode(&bytes)?.at(context.height);
        if action.nonce != council.nonce {
            return refuse(format!("nonce {} is not {}", action.nonce, council.nonce));
        }
        let chain = self.get_chain_tag()?;
        let message = action.signing_message(chain);
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
            CouncilKind::Rotate {
                rotation,
                acceptances,
            } => {
                return Self::schedule_rotation(
                    overlay,
                    council,
                    rotation,
                    acceptances,
                    chain,
                    context,
                );
            }
        };
        let module = Module::from_tag(module_tag)
            .ok_or_else(|| NodeError::Decode(format!("unknown module {module_tag}")))?;
        // Q7 A: Governance holds the vote that answers the council.
        if module == Module::Governance {
            return refuse("the council cannot pause or resume governance".into());
        }
        let record = match action.kind {
            // Returned above; refused here rather than trusted to stay that way.
            CouncilKind::SunsetSuites { .. } | CouncilKind::Rotate { .. } => {
                return refuse("a sunset is not a pause".into());
            }
            CouncilKind::Pause { blocks, .. } => {
                if blocks == 0 || blocks > council.max_pause_blocks {
                    return refuse(format!(
                        "a pause of {blocks} blocks; at most {}",
                        council.max_pause_blocks
                    ));
                }
                // Read from the council record: an automatic trip overwrites
                // the breaker record and must not reset the council's clock.
                if let Some(end) = council.last_pause_end.get(&module_tag) {
                    let open_at = end.saturating_add(COUNCIL_RENEWAL_COOLDOWN_BLOCKS);
                    if context.height < open_at {
                        return refuse(format!(
                            "{} is in its renewal cooldown until height {open_at}",
                            module.label()
                        ));
                    }
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
        council.last_pause_end.insert(module_tag, record.until);
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
        if let Some(open_at) = council
            .last_sunset_at
            .map(|last| last.saturating_add(COUNCIL_RENEWAL_COOLDOWN_BLOCKS))
            .filter(|open_at| context.height < *open_at)
        {
            return Err(NodeError::Network(format!(
                "council action refused: a council sunset is in its renewal cooldown until height {open_at}"
            )));
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
        council.last_sunset_at = Some(context.height);
        council.nonce = council.nonce.saturating_add(1);
        StateDB::put_record(overlay, COUNCIL_KEY.to_vec(), council.encode());
        Ok(())
    }

    /// Records a rotation the council in force approved (founder audit Q8).
    /// Every new member must have signed its acceptance. It replaces any
    /// pending one, and must clear the lead from this block.
    fn schedule_rotation(
        overlay: &mut Overlay,
        mut council: CouncilRecord,
        rotation: &Rotation,
        acceptances: &[Box<[u8; SIGNATURE_LENGTH]>],
        chain: &ChainTag,
        context: BlockContext,
    ) -> Result<()> {
        let refuse = |why: String| {
            Err(NodeError::Network(format!(
                "council action refused: rotation: {why}"
            )))
        };
        if let Err(why) = check_member_set(rotation.threshold, &rotation.members) {
            return refuse(why);
        }
        if acceptances.len() != rotation.members.len() {
            return refuse(format!(
                "{} acceptances for {} members",
                acceptances.len(),
                rotation.members.len()
            ));
        }
        let accepted = rotation.acceptance_message(chain);
        for (i, (key, sig)) in rotation.members.iter().zip(acceptances).enumerate() {
            if VerifyingKey::from_bytes(key)?
                .verify(&accepted, sig)
                .is_err()
            {
                return refuse(format!(
                    "members[{i}] did not sign its acceptance: nobody is shown to hold that key"
                ));
            }
        }
        let earliest = context.height.saturating_add(COUNCIL_ROTATION_LEAD_BLOCKS);
        let latest = context.height.saturating_add(MAX_SCHEDULE_HORIZON_BLOCKS);
        if rotation.activation < earliest || rotation.activation > latest {
            return refuse(format!(
                "activation {} is not from {earliest} to {latest}: the {COUNCIL_ROTATION_LEAD_BLOCKS}-block lead and the horizon",
                rotation.activation
            ));
        }
        council.pending = Some(rotation.clone());
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
            pending: None,
            last_sunset_at: None,
            last_pause_end: BTreeMap::new(),
        };
        assert_eq!(CouncilRecord::decode(&r.encode()).unwrap(), r);
        assert_eq!(r.encode()[0], 1, "no rotation, no sunset: version 1 bytes");
        let extended = CouncilRecord {
            pending: Some(Rotation {
                activation: 99,
                threshold: 1,
                members: vec![[4; PUBLIC_KEY_LEN]],
            }),
            last_sunset_at: Some(7),
            last_pause_end: BTreeMap::from([(Module::Dex.tag(), 40), (Module::Vm.tag(), 41)]),
            ..r.clone()
        };
        assert_eq!(CouncilRecord::decode(&extended.encode()).unwrap(), extended);
        // Version 2 with both fields empty would be a second form of a
        // version 1 record, and is refused.
        let mut empty_v2 = r.encode();
        empty_v2[0] = 2;
        empty_v2.extend_from_slice(&[0; 1 + 8 + 1 + 8]);
        assert!(CouncilRecord::decode(&empty_v2).is_err());
        // Flag 0 with a non-zero height would be a second form too.
        let only_pause = CouncilRecord {
            last_pause_end: BTreeMap::from([(Module::Dex.tag(), 40)]),
            ..r.clone()
        };
        let mut stray = only_pause.encode();
        assert_eq!(stray[0], 2);
        let flag = r.encode().len();
        assert_eq!(stray[flag], 0, "no sunset");
        stray[flag + 1] = 5;
        let err = CouncilRecord::decode(&stray).unwrap_err();
        assert!(err.to_string().contains("stray"), "{err}");
        // Pause ends out of order, repeated, or for an unknown module.
        let pause_at = stray.len() - 9;
        for tag in [u8::MAX, Module::Dex.tag()] {
            let mut bad = only_pause.encode();
            bad.splice(pause_at - 8..pause_at, 2u64.to_le_bytes());
            bad.push(tag);
            bad.extend_from_slice(&41u64.to_le_bytes());
            assert!(CouncilRecord::decode(&bad).is_err(), "tag {tag}");
        }
    }

    #[test]
    fn a_pending_rotation_takes_over_at_its_activation_and_not_before() {
        let r = CouncilRecord {
            threshold: 2,
            max_pause_blocks: 600,
            nonce: 3,
            members: vec![
                [1; PUBLIC_KEY_LEN],
                [2; PUBLIC_KEY_LEN],
                [3; PUBLIC_KEY_LEN],
            ],
            pending: Some(Rotation {
                activation: 50,
                threshold: 1,
                members: vec![[9; PUBLIC_KEY_LEN]],
            }),
            last_sunset_at: None,
            last_pause_end: BTreeMap::new(),
        };
        assert_eq!(r.clone().at(49), r);
        let rotated = r.at(50);
        assert_eq!(rotated.threshold, 1);
        assert_eq!(rotated.members, vec![[9; PUBLIC_KEY_LEN]]);
        assert_eq!((rotated.pending, rotated.nonce), (None, 3));
    }

    /// An automatic trip rewrites the module's breaker record. Before
    /// 2026-11-14's review the cooldown was read from that record, so a trip
    /// during the cooldown let the council pause again at once.
    #[test]
    fn an_automatic_trip_does_not_reset_the_council_renewal_cooldown() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        let chain = ChainTag::from_genesis([7; 32]);
        db.bind_chain(chain).unwrap();
        let keys: Vec<_> = (0..3u8)
            .map(|i| crate::crypto::keys::signing_key_from_seed(&[0x40 + i; 32]).unwrap())
            .collect();
        let mut overlay = Overlay::default();
        let council = CouncilRecord {
            threshold: 2,
            max_pause_blocks: 10,
            nonce: 0,
            members: keys.iter().map(|k| k.verifying_key().to_bytes()).collect(),
            pending: None,
            last_sunset_at: None,
            last_pause_end: BTreeMap::new(),
        };
        StateDB::put_record(&mut overlay, COUNCIL_KEY.to_vec(), council.encode());
        let pause = |nonce: u64| {
            let mut action = CouncilAction {
                kind: CouncilKind::Pause {
                    module: Module::Dex.tag(),
                    blocks: 10,
                },
                nonce,
                approvals: vec![],
            };
            let message = action.signing_message(&chain);
            action.approvals = (0..2u8)
                .map(|i| (i, Box::new(keys[usize::from(i)].sign(&message).unwrap())))
                .collect();
            action
        };
        db.apply_council(&mut overlay, &pause(0), BlockContext::at_height(1))
            .unwrap();
        // The council pause ends at 11; an automatic trip lands at 20.
        StateDB::trip_breaker(&mut overlay, Module::Dex, Invariant::PoolValuePerShare, 20);
        let open_at = 11 + COUNCIL_RENEWAL_COOLDOWN_BLOCKS;
        let err = db
            .apply_council(
                &mut overlay,
                &pause(1),
                BlockContext::at_height(open_at - 1),
            )
            .unwrap_err();
        assert!(err.to_string().contains("cooldown"), "{err}");
        db.apply_council(&mut overlay, &pause(1), BlockContext::at_height(open_at))
            .unwrap();
    }
}
