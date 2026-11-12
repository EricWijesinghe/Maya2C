//! Account authority and guardian recovery (ADR-086 §3): an address signed
//! for by a stored policy, changeable without changing the address.
//!
//! Three records under `ka:`, a layer of the state root (invariant 25), folded
//! only when non-empty, so a chain where nobody opts in keeps its root byte for
//! byte. Their writes are overlay records, so the undo journal restores them on
//! a reorg exactly as it does every generic record.
//!
//! - `ka:a:<address>` — the policy digest that signs for the address, and a
//!   replacement queued behind the guardian delay;
//! - `ka:g:<address>` — the guardian set, and a queued replacement;
//! - `ka:r:<address>` — the one open recovery: each guardian's approval (one
//!   digest per guardian, so a later proposal *moves* it — F6), the proposed
//!   policies, and the height the leading digest reached quorum.
//!
//! **Rule A.** With `ka:a:<X>` present, only a wire-version-9 frame naming `X`
//! and presenting a policy whose digest equals the record acts for `X`. Every
//! v5–v8 frame whose derived sender is `X` is refused, including one signed by
//! the original key: that is what voids a thief's pre-signed transactions once
//! recovery finishes. **Rule B.** With no record, nothing changes — and a v9
//! frame is refused, since it names an account nobody bound it to.
//!
//! A queued change takes effect by height, not by transaction, as in the
//! vault: whoever reads the record at or after maturity sees the new value.
//! No action here writes a balance.

use std::collections::BTreeMap;

use maya_crypto_pq::multisig::MultisigPolicy;

use crate::core::Transaction;
use crate::core::TxKind;
use crate::core::account_payload::{AccountAction, GuardianSet, MAX_GUARDIANS};
use crate::core::account_tx::signer_policy;
use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

/// The account-authority layer's prefix.
pub(crate) const AUTHORITY_PREFIX: &[u8] = b"ka:";
const AUTHORITY_RECORD: &[u8] = b"ka:a:";
const GUARDIAN_RECORD: &[u8] = b"ka:g:";
const RECOVERY_RECORD: &[u8] = b"ka:r:";

/// Shortest guardian delay: one mainnet epoch of 3,600 blocks (ADR-086 §3.3).
pub const MIN_DELAY_BLOCKS: u64 = 3_600;
/// Longest guardian delay: the vault's, the longest ADR-082's council lead
/// covers, so no suite can be sunset under a recovery still waiting.
pub const MAX_DELAY_BLOCKS: u64 = crate::state::vault::MAX_DELAY_BLOCKS;

const RECORD_VERSION: u8 = 1;

/// Who signs for an address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorityRecord {
    /// `MultisigPolicy::digest` of the policy in force.
    pub policy_digest: [u8; 32],
    /// Height that policy took effect.
    pub since: u64,
    /// A replacement digest and the height it takes effect.
    pub queued: Option<([u8; 32], u64)>,
}

/// An account's guardians.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardianRecord {
    /// The set in force (before any queued replacement matures).
    pub set: GuardianSet,
    /// A replacement and the height it takes effect.
    pub queued: Option<(GuardianSet, u64)>,
}

/// The open recovery of an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryRecord {
    /// Height the request opened.
    pub opened_at: u64,
    /// Each approving guardian and the digest it approves.
    pub approvals: BTreeMap<[u8; 32], [u8; 32]>,
    /// Every digest some approval names, with its policy.
    pub policies: BTreeMap<[u8; 32], MultisigPolicy>,
    /// The digest holding quorum and the height it reached it.
    pub quorum: Option<([u8; 32], u64)>,
}

fn refuse<T>(why: impl Into<String>) -> Result<T> {
    Err(NodeError::Account(why.into()))
}

fn key(prefix: &[u8], address: &Address) -> Vec<u8> {
    [prefix, address.as_slice()].concat()
}

fn check_version(r: &mut ByteReader<'_>, what: &str) -> Result<()> {
    if r.read_u8()? != RECORD_VERSION {
        return Err(NodeError::Decode(format!("{what} record version")));
    }
    Ok(())
}

fn put_option_tag(out: &mut Vec<u8>, present: bool) {
    out.push(u8::from(present));
}

fn read_option_tag(r: &mut ByteReader<'_>, what: &str) -> Result<bool> {
    match r.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        t => Err(NodeError::Decode(format!("{what} option tag {t}"))),
    }
}

impl AuthorityRecord {
    /// The digest in force at `height`.
    #[must_use]
    pub fn effective(&self, height: u64) -> [u8; 32] {
        match self.queued {
            Some((next, ready)) if ready <= height => next,
            _ => self.policy_digest,
        }
    }

    fn normalised(self, height: u64) -> Self {
        match self.queued {
            Some((next, ready)) if ready <= height => Self {
                policy_digest: next,
                since: ready,
                queued: None,
            },
            _ => self,
        }
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![RECORD_VERSION];
        out.extend_from_slice(&self.policy_digest);
        out.extend_from_slice(&self.since.to_le_bytes());
        put_option_tag(&mut out, self.queued.is_some());
        if let Some((next, ready)) = &self.queued {
            out.extend_from_slice(next);
            out.extend_from_slice(&ready.to_le_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        check_version(&mut r, "authority")?;
        let policy_digest = r.read_array()?;
        let since = r.read_u64()?;
        let queued = if read_option_tag(&mut r, "authority")? {
            Some((r.read_array()?, r.read_u64()?))
        } else {
            None
        };
        r.finish()?;
        Ok(Self {
            policy_digest,
            since,
            queued,
        })
    }
}

impl GuardianRecord {
    /// The set in force at `height`.
    #[must_use]
    pub fn effective(&self, height: u64) -> &GuardianSet {
        match &self.queued {
            Some((next, ready)) if *ready <= height => next,
            _ => &self.set,
        }
    }

    fn normalised(self, height: u64) -> Self {
        match self.queued {
            Some((next, ready)) if ready <= height => Self {
                set: next,
                queued: None,
            },
            queued => Self { queued, ..self },
        }
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![RECORD_VERSION];
        self.set.encode_into(&mut out);
        put_option_tag(&mut out, self.queued.is_some());
        if let Some((next, ready)) = &self.queued {
            next.encode_into(&mut out);
            out.extend_from_slice(&ready.to_le_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        check_version(&mut r, "guardian")?;
        let set = GuardianSet::decode(&mut r)?;
        let queued = if read_option_tag(&mut r, "guardian")? {
            Some((GuardianSet::decode(&mut r)?, r.read_u64()?))
        } else {
            None
        };
        r.finish()?;
        Ok(Self { set, queued })
    }
}

impl RecoveryRecord {
    fn opened(height: u64) -> Self {
        Self {
            opened_at: height,
            approvals: BTreeMap::new(),
            policies: BTreeMap::new(),
            quorum: None,
        }
    }

    /// Approvals for `digest` from members of `set` (F4: removed guardians
    /// count for nothing).
    #[must_use]
    pub fn count(&self, digest: &[u8; 32], set: &GuardianSet) -> usize {
        self.approvals
            .iter()
            .filter(|(g, d)| *d == digest && set.guardians.contains(g))
            .count()
    }

    /// Drops approvals from non-members and policies nobody approves, then
    /// records which digest holds quorum. A strict majority means at most one
    /// can. Quorum kept by the same digest keeps its height; a new one starts
    /// its clock now.
    fn retally(&mut self, set: &GuardianSet, height: u64) {
        self.approvals.retain(|g, _| set.guardians.contains(g));
        let approvals = &self.approvals;
        self.policies
            .retain(|d, _| approvals.values().any(|approved| approved == d));
        let leading = self
            .policies
            .keys()
            .find(|d| self.count(d, set) >= usize::from(set.threshold))
            .copied();
        self.quorum = match (leading, self.quorum) {
            (Some(d), Some((q, since))) if q == d => Some((q, since)),
            (Some(d), _) => Some((d, height)),
            (None, _) => None,
        };
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![RECORD_VERSION];
        out.extend_from_slice(&self.opened_at.to_le_bytes());
        put_option_tag(&mut out, self.quorum.is_some());
        if let Some((digest, since)) = &self.quorum {
            out.extend_from_slice(digest);
            out.extend_from_slice(&since.to_le_bytes());
        }
        // Both maps are bounded by MAX_GUARDIANS: approvals come only from
        // current guardians and retally drops the rest.
        out.push(u8::try_from(self.policies.len()).unwrap_or(u8::MAX));
        for policy in self.policies.values() {
            out.extend_from_slice(&policy.encode());
        }
        out.push(u8::try_from(self.approvals.len()).unwrap_or(u8::MAX));
        for (guardian, digest) in &self.approvals {
            out.extend_from_slice(guardian);
            out.extend_from_slice(digest);
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        check_version(&mut r, "recovery")?;
        let opened_at = r.read_u64()?;
        let quorum = if read_option_tag(&mut r, "recovery")? {
            Some((r.read_array()?, r.read_u64()?))
        } else {
            None
        };
        let policy_count = usize::from(r.read_u8()?);
        let approval_count = |n: usize| n <= MAX_GUARDIANS;
        if !approval_count(policy_count) {
            return Err(NodeError::Decode(format!(
                "{policy_count} recovery policies"
            )));
        }
        let mut policies = BTreeMap::new();
        for _ in 0..policy_count {
            let (policy, used) = MultisigPolicy::decode(r.peek_remaining())
                .map_err(|e| NodeError::Decode(e.to_string()))?;
            r.read_slice(used)?;
            policies.insert(policy.digest(), policy);
        }
        let n = usize::from(r.read_u8()?);
        if !approval_count(n) {
            return Err(NodeError::Decode(format!("{n} recovery approvals")));
        }
        let mut approvals = BTreeMap::new();
        for _ in 0..n {
            let guardian: [u8; 32] = r.read_array()?;
            let digest: [u8; 32] = r.read_array()?;
            if !policies.contains_key(&digest) {
                return Err(NodeError::Decode("approval for an unknown policy".into()));
            }
            approvals.insert(guardian, digest);
        }
        r.finish()?;
        Ok(Self {
            opened_at,
            approvals,
            policies,
            quorum,
        })
    }
}

/// Refuses a guardian set outside §3.3's bounds: 1–7 distinct guardians, none
/// the account itself, a strict majority, and a delay inside the compiled
/// range (F3: threshold 0, threshold > n and delay 0 are all refused).
fn validate(set: &GuardianSet, account: &Address) -> Result<()> {
    let n = set.guardians.len();
    if !(1..=MAX_GUARDIANS).contains(&n) {
        return refuse(format!(
            "{n} guardians; an account needs 1..={MAX_GUARDIANS}"
        ));
    }
    for (i, g) in set.guardians.iter().enumerate() {
        if g == account {
            return refuse("an account cannot be its own guardian");
        }
        if set.guardians[..i].contains(g) {
            return refuse("a guardian is named twice");
        }
    }
    let t = usize::from(set.threshold);
    if t > n || 2 * t <= n {
        return refuse(format!(
            "threshold {t} of {n} is not a strict majority of the guardians"
        ));
    }
    if !(MIN_DELAY_BLOCKS..=MAX_DELAY_BLOCKS).contains(&set.delay_blocks) {
        return refuse(format!(
            "delay of {} blocks is outside {MIN_DELAY_BLOCKS}..={MAX_DELAY_BLOCKS}",
            set.delay_blocks
        ));
    }
    Ok(())
}

/// Refuses a policy any of whose suites may not sign at every one of
/// `heights`: a recovery to keys the chain retires before it can finish
/// would recover to nothing.
fn check_suites(policy: &MultisigPolicy, context: BlockContext, heights: &[u64]) -> Result<()> {
    let suites = crate::crypto::suites::verification_policy(&context.suite_sunsets)?;
    for k in policy.keys() {
        for &h in heights {
            crate::crypto::suites::check_admissible(&suites, k.suite, h)
                .map_err(|e| NodeError::Account(format!("new policy: {e}")))?;
        }
    }
    Ok(())
}

impl StateDB {
    fn authority_record(&self, overlay: &Overlay, a: &Address) -> Result<Option<AuthorityRecord>> {
        self.record(overlay, &key(AUTHORITY_RECORD, a))?
            .map(|b| AuthorityRecord::decode(&b))
            .transpose()
    }

    fn guardian_record(&self, overlay: &Overlay, a: &Address) -> Result<Option<GuardianRecord>> {
        self.record(overlay, &key(GUARDIAN_RECORD, a))?
            .map(|b| GuardianRecord::decode(&b))
            .transpose()
    }

    fn recovery_record(&self, overlay: &Overlay, a: &Address) -> Result<Option<RecoveryRecord>> {
        self.record(overlay, &key(RECOVERY_RECORD, a))?
            .map(|b| RecoveryRecord::decode(&b))
            .transpose()
    }

    /// The committed authority, guardian and recovery records of `address`,
    /// for RPC.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub fn committed_account_authority(
        &self,
        address: &Address,
    ) -> Result<(
        Option<AuthorityRecord>,
        Option<GuardianRecord>,
        Option<RecoveryRecord>,
    )> {
        let empty = Overlay::default();
        Ok((
            self.authority_record(&empty, address)?,
            self.guardian_record(&empty, address)?,
            self.recovery_record(&empty, address)?,
        ))
    }

    /// Rules A and B, and the activation gate for wire version 9 and
    /// `TxKind::Account`. Reads one record and writes nothing, so staging runs
    /// it before the signature and the fee, and the mempool runs it at the tip.
    ///
    /// # Errors
    ///
    /// [`NodeError::Account`] below activation; [`NodeError::AuthorityMoved`]
    /// when the frame is not the one the record admits.
    pub(crate) fn authority_check(
        &self,
        overlay: &Overlay,
        tx: &Transaction,
        sender: &Address,
        height: u64,
        activation: u64,
    ) -> Result<()> {
        let bound = tx.multisig.as_deref().filter(|a| a.account().is_some());
        if height < activation {
            if bound.is_some() {
                return refuse(format!("wire version 9 is not active at height {height}"));
            }
            if matches!(tx.kind, TxKind::Account(_)) {
                return refuse(format!("account actions are not active at height {height}"));
            }
            // Below activation no `ka:` record can exist: skip the read.
            return Ok(());
        }
        let record = self.authority_record(overlay, sender)?;
        let moved = |why: String| Err(NodeError::AuthorityMoved(why));
        match (bound, record) {
            (None, None) => Ok(()),
            (None, Some(_)) => moved(format!(
                "{} is signed for by a stored policy; only a v9 frame presenting it acts for it",
                hex::encode(sender)
            )),
            (Some(_), None) => moved(format!(
                "{} has no authority record, so no v9 frame can name it",
                hex::encode(sender)
            )),
            (Some(auth), Some(r)) if r.effective(height) == auth.policy().digest() => Ok(()),
            (Some(_), Some(_)) => moved(format!(
                "the policy presented is not the one signing for {}",
                hex::encode(sender)
            )),
        }
    }

    /// Executes an account action carried by `tx`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Account`] for a refused action; storage errors.
    pub(crate) fn apply_account(
        &self,
        overlay: &mut Overlay,
        tx: &Transaction,
        action: &AccountAction,
        context: BlockContext,
    ) -> Result<()> {
        if !context.account_authority_active() {
            return refuse(format!(
                "account actions are not active at height {}",
                context.height
            ));
        }
        let sender = tx.sender();
        match action {
            AccountAction::AdoptPolicy(policy) => {
                self.adopt_policy(overlay, &sender, policy, context)
            }
            AccountAction::SetGuardians(set) => {
                self.set_guardians(overlay, tx, &sender, set, context)
            }
            AccountAction::ProposeRecovery { account, policy } => {
                self.propose_recovery(overlay, &sender, account, policy, context)
            }
            AccountAction::CancelRecovery { account } => {
                self.cancel_recovery(overlay, &sender, account, context.height)
            }
            AccountAction::FinishRecovery { account } => {
                self.finish_recovery(overlay, account, context)
            }
            AccountAction::WithdrawApproval { account } => {
                self.withdraw_approval(overlay, &sender, account, context.height)
            }
        }
    }

    fn adopt_policy(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        policy: &MultisigPolicy,
        context: BlockContext,
    ) -> Result<()> {
        let height = context.height;
        let digest = policy.digest();
        let delay = self
            .guardian_record(overlay, owner)?
            .map(|g| g.effective(height).delay_blocks);
        let ready = delay.map(|d| height.saturating_add(d));
        check_suites(policy, context, &[height, ready.unwrap_or(height)])?;
        let record = match (self.authority_record(overlay, owner)?, ready) {
            // Guardians set: queued behind the current delay, so one stolen
            // quorum cannot lock the owner out before the owner can react.
            (Some(existing), Some(ready)) => AuthorityRecord {
                queued: Some((digest, ready)),
                ..existing.normalised(height)
            },
            _ => AuthorityRecord {
                policy_digest: digest,
                since: height,
                queued: None,
            },
        };
        StateDB::put_record(overlay, key(AUTHORITY_RECORD, owner), record.encode());
        Ok(())
    }

    fn set_guardians(
        &self,
        overlay: &mut Overlay,
        tx: &Transaction,
        owner: &Address,
        set: &GuardianSet,
        context: BlockContext,
    ) -> Result<()> {
        let height = context.height;
        validate(set, owner)?;
        if self.authority_record(overlay, owner)?.is_none() {
            // Opting in: the record is the policy that signed this (Rule B).
            let record = AuthorityRecord {
                policy_digest: signer_policy(tx)?.digest(),
                since: height,
                queued: None,
            };
            StateDB::put_record(overlay, key(AUTHORITY_RECORD, owner), record.encode());
        }
        let record = match self.guardian_record(overlay, owner)? {
            None => GuardianRecord {
                set: set.clone(),
                queued: None,
            },
            // F1: a later change waits out the *current* delay.
            Some(existing) => {
                let existing = existing.normalised(height);
                let ready = height.saturating_add(existing.set.delay_blocks);
                GuardianRecord {
                    queued: Some((set.clone(), ready)),
                    ..existing
                }
            }
        };
        StateDB::put_record(overlay, key(GUARDIAN_RECORD, owner), record.encode());
        Ok(())
    }

    fn current_guardians(
        &self,
        overlay: &Overlay,
        account: &Address,
        height: u64,
    ) -> Result<GuardianSet> {
        match self.guardian_record(overlay, account)? {
            Some(g) => Ok(g.effective(height).clone()),
            None => refuse("the account has no guardians"),
        }
    }

    fn propose_recovery(
        &self,
        overlay: &mut Overlay,
        guardian: &Address,
        account: &Address,
        policy: &MultisigPolicy,
        context: BlockContext,
    ) -> Result<()> {
        let height = context.height;
        let set = self.current_guardians(overlay, account, height)?;
        if !set.guardians.contains(guardian) {
            return refuse("only a current guardian may propose a recovery");
        }
        let digest = policy.digest();
        let current = self
            .authority_record(overlay, account)?
            .ok_or_else(|| NodeError::InvariantViolation("guardians without authority".into()))?;
        if current.effective(height) == digest {
            return refuse("the proposed policy already signs for the account");
        }
        check_suites(
            policy,
            context,
            &[height, height.saturating_add(set.delay_blocks)],
        )?;
        let mut recovery = self
            .recovery_record(overlay, account)?
            .unwrap_or_else(|| RecoveryRecord::opened(height));
        // F6: one approval per guardian, following its latest proposal.
        recovery.approvals.insert(*guardian, digest);
        recovery.policies.insert(digest, policy.clone());
        recovery.retally(&set, height);
        StateDB::put_record(overlay, key(RECOVERY_RECORD, account), recovery.encode());
        Ok(())
    }

    fn cancel_recovery(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        account: &Address,
        height: u64,
    ) -> Result<()> {
        if sender != account {
            return refuse("only the account's own authority may cancel");
        }
        let recovery = self.recovery_record(overlay, account)?;
        let authority = self
            .authority_record(overlay, account)?
            .map(|a| a.normalised(height));
        let guardians = self
            .guardian_record(overlay, account)?
            .map(|g| g.normalised(height));
        let queued = authority.as_ref().is_some_and(|a| a.queued.is_some())
            || guardians.as_ref().is_some_and(|g| g.queued.is_some());
        if recovery.is_none() && !queued {
            return refuse("no recovery is open and no change is queued");
        }
        StateDB::delete_record(overlay, key(RECOVERY_RECORD, account));
        if let Some(a) = authority {
            let a = AuthorityRecord { queued: None, ..a };
            StateDB::put_record(overlay, key(AUTHORITY_RECORD, account), a.encode());
        }
        if let Some(g) = guardians {
            let g = GuardianRecord { queued: None, ..g };
            StateDB::put_record(overlay, key(GUARDIAN_RECORD, account), g.encode());
        }
        Ok(())
    }

    fn finish_recovery(
        &self,
        overlay: &mut Overlay,
        account: &Address,
        context: BlockContext,
    ) -> Result<()> {
        let height = context.height;
        let Some(recovery) = self.recovery_record(overlay, account)? else {
            return refuse("no recovery is open");
        };
        let guardians = self
            .guardian_record(overlay, account)?
            .ok_or_else(|| NodeError::InvariantViolation("recovery without guardians".into()))?
            .normalised(height);
        let set = &guardians.set;
        let Some((digest, since)) = recovery.quorum else {
            return refuse("the recovery has no quorum");
        };
        // Counted against the guardians in force now, not when quorum formed.
        if recovery.count(&digest, set) < usize::from(set.threshold) {
            return refuse("the recovery's quorum is no longer met by the current guardians");
        }
        let ready = since.saturating_add(set.delay_blocks);
        if height < ready {
            return refuse(format!("the recovery matures at {ready}, not {height}"));
        }
        let policy = recovery
            .policies
            .get(&digest)
            .ok_or_else(|| NodeError::InvariantViolation("quorum on an unknown policy".into()))?;
        check_suites(policy, context, &[height])?;
        let authority = AuthorityRecord {
            policy_digest: digest,
            since: height,
            queued: None,
        };
        StateDB::put_record(overlay, key(AUTHORITY_RECORD, account), authority.encode());
        let guardians = GuardianRecord {
            queued: None,
            ..guardians
        };
        StateDB::put_record(overlay, key(GUARDIAN_RECORD, account), guardians.encode());
        StateDB::delete_record(overlay, key(RECOVERY_RECORD, account));
        Ok(())
    }

    fn withdraw_approval(
        &self,
        overlay: &mut Overlay,
        guardian: &Address,
        account: &Address,
        height: u64,
    ) -> Result<()> {
        let Some(mut recovery) = self.recovery_record(overlay, account)? else {
            return refuse("no recovery is open");
        };
        if recovery.approvals.remove(guardian).is_none() {
            return refuse("the sender has no approval to withdraw");
        }
        let set = self.current_guardians(overlay, account, height)?;
        recovery.retally(&set, height);
        let k = key(RECOVERY_RECORD, account);
        if recovery.approvals.is_empty() {
            StateDB::delete_record(overlay, k);
        } else {
            StateDB::put_record(overlay, k, recovery.encode());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn set(n: u8, t: u8, delay: u64) -> GuardianSet {
        GuardianSet {
            guardians: (1..=n).map(|i| [i; 32]).collect(),
            threshold: t,
            delay_blocks: delay,
        }
    }

    #[test]
    fn records_round_trip_and_mature_by_height() {
        let a = AuthorityRecord {
            policy_digest: [1; 32],
            since: 5,
            queued: Some(([2; 32], 50)),
        };
        assert_eq!(AuthorityRecord::decode(&a.encode()).unwrap(), a);
        assert_eq!(a.effective(49), [1; 32]);
        assert_eq!(a.effective(50), [2; 32]);
        let g = GuardianRecord {
            set: set(3, 2, MIN_DELAY_BLOCKS),
            queued: Some((set(5, 3, MIN_DELAY_BLOCKS), 9)),
        };
        assert_eq!(GuardianRecord::decode(&g.encode()).unwrap(), g);
        assert_eq!(g.effective(9).threshold, 3);
    }

    #[test]
    fn guardian_bounds_are_a_strict_majority_inside_the_delay_range() {
        let me = [0xAA; 32];
        assert!(validate(&set(3, 2, MIN_DELAY_BLOCKS), &me).is_ok());
        assert!(validate(&set(3, 0, MIN_DELAY_BLOCKS), &me).is_err());
        assert!(validate(&set(4, 2, MIN_DELAY_BLOCKS), &me).is_err());
        assert!(validate(&set(3, 4, MIN_DELAY_BLOCKS), &me).is_err());
        assert!(validate(&set(3, 2, MIN_DELAY_BLOCKS - 1), &me).is_err());
        assert!(validate(&set(3, 2, MAX_DELAY_BLOCKS + 1), &me).is_err());
        assert!(validate(&set(0, 0, MIN_DELAY_BLOCKS), &me).is_err());
        let mut own = set(3, 2, MIN_DELAY_BLOCKS);
        own.guardians[0] = me;
        assert!(validate(&own, &me).is_err());
        let mut twice = set(3, 2, MIN_DELAY_BLOCKS);
        twice.guardians[1] = twice.guardians[0];
        assert!(validate(&twice, &me).is_err());
    }
}
