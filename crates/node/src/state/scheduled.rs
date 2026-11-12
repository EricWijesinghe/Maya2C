//! Scheduled payments (ADR-135 P-7, feature bit 46; ADR-109 rules, top-up cut).
//!
//! ## State (invariant 25: one prefix, one layer, `StateLayer::Scheduled`)
//!
//! ```text
//! sp:s: ‖ schedule_id(32)                  → 0x01 ‖ payer(32) ‖ payee(32) ‖ amount(32 LE) ‖ next_due(8 LE) ‖ interval(8 LE) ‖ remaining(4 LE) ‖ escrow(32 LE) ‖ missed(1)   (150 B)
//! sp:d: ‖ next_due(8 BE) ‖ schedule_id(32) → (empty)   the due index
//! schedule_id = blake3::derive_key("maya2c schedule v1", payer ‖ nonce(8 LE))
//! ```
//!
//! ## The end-of-block step (threat model S-1, S-2; invariant 33)
//!
//! After every transaction of the block (so a cancel in the due block wins,
//! S-7), [`StateDB::settle_schedules`] walks `sp:d:` in key order — big-endian
//! height, then id, so `(next_due, id)` order — while `next_due <= height`,
//! at most [`MAX_DUE_PER_BLOCK`] rows. The rest is still due at the next block
//! and sorts first there. It is the bounded-prune pattern of
//! `state::names::settle_names`. A run whose escrow is short of
//! `amount + RUN_FEE` is a counted miss, never an error, so a schedule can
//! never void the block it is due in (invariant 7). Every write goes through
//! the block's overlay, so the undo journal restores it on a reorg and the
//! declared `state_root` must reproduce it (invariant 24).
//!
//! The step reads committed state, the overlay and the height only: no clock,
//! no float, and a `BTreeMap`/key-ordered scan, never a hash-map order.
//!
//! **Residual, priced (S-2, both reviews 2026-11-12; Eric decided 2026-11-12).**
//! The step sustains `256 × MIN_INTERVAL = 921,600` minimum-interval
//! schedules. Above that the backlog grows, and because a run reschedules at
//! `next_due + interval`, an older backlog keeps sorting first: honest
//! schedules wait behind it. Transactions are never delayed. `RUN_FEE` now
//! starts non-zero (`SCHEDULE_RUN_FEE_DEFAULT`, one minimal transfer's byte
//! fee per run, prepaid), so stuffing the queue costs a transfer's fee per
//! queued run instead of one base unit. It is still governed: a proposal can
//! raise it if a backlog appears, or lower it, back to zero, which reopens
//! this residual.
//!
//! **Catch-up after a breaker.** A hold longer than `interval` leaves
//! `next_due + interval <= height`, so such a schedule runs again on the next
//! blocks until it is current: deterministic, prepaid and conserved; intended,
//! since a halt delays payments rather than cancelling them.
//!
//! ## Money math, with its bounds (no 512-bit intermediate)
//!
//! - `unit = amount.checked_add(RUN_FEE)`, refused unless `unit < 2^249`;
//! - `escrow = unit.checked_mul(runs)` with `runs <= MAX_RUNS = 120 < 2^7`, so
//!   the product is below `2^256` whenever the check above passed;
//! - a run: `escrow >= unit` first, then `checked_sub` (S-3).
//!
//! `RUN_FEE` is the governed parameter `ScheduleRunFee` (invariant 17, tag
//! 16, `SCHEDULE_RUN_FEE_DEFAULT` at activation, below `2^252` by its bounds), read from state per block, never
//! a `const`. It is paid to `FEE_COLLECTOR`, so it joins the tips.
//!
//! ## Vaults (invariant 32)
//!
//! A vault account may send only a plain transfer or a vault action, so it
//! cannot create or cancel a schedule (`vault_check` refuses the form). A run
//! moves escrow the chain already holds, so it is never vault outflow.

use maya_governance::limits::MAX_SCHEDULE_HORIZON_BLOCKS;
use maya_governance::params::ParameterKey;
use maya_stateless_core::amount::to_u256;
use primitive_types::U256;

use crate::core::TxKind;
use crate::core::codec::{ByteReader, put_amount};
use crate::core::schedule_payload::ScheduleAction;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::fees::FEE_COLLECTOR;
use crate::state::invariant_guard::Module;
use crate::state::shielded::FEE_SINK;

/// Prefix of every scheduled-payment row. One committed layer.
pub const SCHEDULED_PREFIX: &[u8] = b"sp:";
pub(crate) const RECORD_PREFIX: &[u8] = b"sp:s:";
const DUE_PREFIX: &[u8] = b"sp:d:";
const ID_DOMAIN: &str = "maya2c schedule v1";
const RECORD_VERSION: u8 = 0x01;
/// Encoded length of a [`ScheduleRecord`].
pub const RECORD_LEN: usize = 1 + 32 + 32 + 32 + 8 + 8 + 4 + 32 + 1;
const DUE_KEY_LEN: usize = 5 + 8 + 32;

/// Fewest blocks between two runs: one hour at one block per second (ADR-135
/// O-1, O-6: compiled).
pub const MIN_INTERVAL: u64 = 3_600;
/// Most runs one schedule may prepay (ADR-135 O-6: compiled).
pub const MAX_RUNS: u32 = 120;
/// Consecutive misses after which a schedule closes and refunds its payer.
pub const MAX_MISSED: u8 = 3;
/// Most due runs one block executes (ADR-135 P-7, invariant 33). The rest
/// carries to the next block in `(next_due, id)` order.
pub const MAX_DUE_PER_BLOCK: usize = 256;
/// `amount + RUN_FEE` must be below `2^UNIT_BOUND_BITS`.
const UNIT_BOUND_BITS: usize = 249;

// The product bound: `unit < 2^249` and `runs < 2^(256 - 249)` keep
// `unit * runs` inside U256, so the `checked_mul` cannot fail once the
// `unit` check passed.
const _: () = assert!((MAX_RUNS as u128) < (1u128 << (256 - UNIT_BOUND_BITS)));

/// Upper bound on the bytes one run writes: its record, two due-index keys
/// (the old one deleted, the new one written) and three accounts (payee,
/// fee collector, payer refund), each a 32-byte address and a 40-byte account.
const WRITE_BYTES_PER_RUN: u64 = (5 + 32 + RECORD_LEN as u64) + 2 * DUE_KEY_LEN as u64 + 3 * 72;
// Invariant 33: the step's state writes in one block stay well inside what a
// full block of transactions may write, so the cap, not the backlog, bounds
// the block's work.
const _: () =
    assert!(MAX_DUE_PER_BLOCK as u64 * WRITE_BYTES_PER_RUN <= crate::state::fees::MAX_BLOCK_BYTES);

/// One schedule, as stored under `sp:s:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduleRecord {
    /// Account that funded the escrow and receives what is left of it.
    pub payer: Address,
    /// Account each run credits.
    pub payee: Address,
    /// Native units per run.
    pub amount: U256,
    /// Height of the next run.
    pub next_due: u64,
    /// Blocks between runs.
    pub interval: u64,
    /// Runs still to pay.
    pub remaining: u32,
    /// Prepaid native units the chain holds for this schedule.
    pub escrow: U256,
    /// Consecutive runs missed for a short escrow.
    pub missed: u8,
}

impl ScheduleRecord {
    /// The 150-byte stored form.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(RECORD_LEN);
        out.push(RECORD_VERSION);
        out.extend_from_slice(&self.payer);
        out.extend_from_slice(&self.payee);
        put_amount(&mut out, &self.amount);
        out.extend_from_slice(&self.next_due.to_le_bytes());
        out.extend_from_slice(&self.interval.to_le_bytes());
        out.extend_from_slice(&self.remaining.to_le_bytes());
        put_amount(&mut out, &self.escrow);
        out.push(self.missed);
        out
    }

    /// Decodes the stored form.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for another length or version byte.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != RECORD_LEN || bytes[0] != RECORD_VERSION {
            return Err(NodeError::Decode("schedule record".into()));
        }
        let mut reader = ByteReader::new(&bytes[1..]);
        let record = Self {
            payer: reader.read_array::<32>()?,
            payee: reader.read_array::<32>()?,
            amount: reader.read_amount()?,
            next_due: reader.read_u64()?,
            interval: reader.read_u64()?,
            remaining: reader.read_u32()?,
            escrow: reader.read_amount()?,
            missed: reader.read_u8()?,
        };
        reader.finish()?;
        Ok(record)
    }
}

/// The id of the schedule `payer` creates with transaction nonce `nonce`.
#[must_use]
pub fn schedule_id(payer: &Address, nonce: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(ID_DOMAIN);
    hasher.update(payer);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Whether bit 46 is active in this block. Admission calls the same check.
///
/// # Errors
///
/// [`NodeError::ScheduleInactive`] for tags 67-68 below the activation height.
pub fn require_scheduled_active(kind: &TxKind, context: BlockContext) -> Result<()> {
    if matches!(kind, TxKind::Schedule(_)) && !context.scheduled_active() {
        return Err(NodeError::ScheduleInactive {
            height: context.height,
            activation: context.scheduled_activation,
        });
    }
    Ok(())
}

const fn refused(reason: &'static str) -> NodeError {
    NodeError::ScheduleRefused { reason }
}

fn corrupt(what: &str) -> NodeError {
    NodeError::Decode(format!("scheduled payments: {what}"))
}

fn record_key(id: &[u8; 32]) -> Vec<u8> {
    [RECORD_PREFIX, &id[..]].concat()
}

fn due_key(next_due: u64, id: &[u8; 32]) -> Vec<u8> {
    [DUE_PREFIX, &next_due.to_be_bytes()[..], &id[..]].concat()
}

/// Splits a due-index key into its height and schedule id. Refused, not
/// skipped: only this module writes these keys, so a malformed one is corrupt
/// state, and stopping beats guessing.
fn parse_due_key(key: &[u8]) -> Result<(u64, [u8; 32])> {
    let rest = key
        .strip_prefix(DUE_PREFIX)
        .filter(|rest| rest.len() == 8 + 32)
        .ok_or_else(|| corrupt("due index key"))?;
    let (due, id) = rest.split_at(8);
    let due = u64::from_be_bytes(due.try_into().map_err(|_| corrupt("due index key"))?);
    let id = id.try_into().map_err(|_| corrupt("due index key"))?;
    Ok((due, id))
}

/// The fields of a `CreateSchedule`, checked.
struct Plan {
    payee: Address,
    amount: U256,
    first_due: u64,
    interval: u64,
    runs: u32,
}

impl Plan {
    /// The creation rules that need no state (ADR-135 P-7; S-14).
    fn validate(&self, height: u64) -> Result<()> {
        if self.payee == FEE_COLLECTOR || self.payee == FEE_SINK {
            return Err(refused(
                "the payee may not be the fee collector, the fee sink or the zero address",
            ));
        }
        if self.amount.is_zero() {
            // A zero amount at a zero RUN_FEE would be a free, permanent row
            // in the due index: queue stuffing without the prepaid cost S-1
            // relies on.
            return Err(refused("the amount must be above zero"));
        }
        if self.first_due <= height {
            return Err(refused(
                "first_due must be above the creating block's height",
            ));
        }
        if self.interval < MIN_INTERVAL {
            return Err(refused("the interval is below MIN_INTERVAL (3600 blocks)"));
        }
        if !(1..=MAX_RUNS).contains(&self.runs) {
            return Err(refused("runs are 1 to MAX_RUNS (120)"));
        }
        // Without a horizon a schedule could sit for 2^62 blocks: two
        // permanent rows for one fee (security review M1). One year, the
        // bound the upgrade schedule already uses.
        if self.first_due - height > MAX_SCHEDULE_HORIZON_BLOCKS
            || self.interval > MAX_SCHEDULE_HORIZON_BLOCKS
        {
            return Err(refused(
                "first_due and interval are within the one-year horizon",
            ));
        }
        // The last scheduled height must exist, so rescheduling a run that
        // was not missed can never overflow.
        self.interval
            .checked_mul(u64::from(self.runs - 1))
            .and_then(|span| self.first_due.checked_add(span))
            .map(|_| ())
            .ok_or_else(|| refused("the last run's height overflows u64"))
    }
}

impl StateDB {
    /// A schedule, from committed state, or `None`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure; a decode error for a corrupt row.
    pub fn get_schedule(&self, id: &[u8; 32]) -> Result<Option<ScheduleRecord>> {
        self.raw_get(&record_key(id))?
            .map(|bytes| ScheduleRecord::decode(&bytes))
            .transpose()
    }

    /// Whether the committed due index holds `(next_due, id)`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure.
    pub fn is_due_indexed(&self, next_due: u64, id: &[u8; 32]) -> Result<bool> {
        Ok(self.raw_get(&due_key(next_due, id))?.is_some())
    }

    /// The governed per-run fee in force (invariant 17).
    fn run_fee(&self, overlay: &Overlay) -> Result<U256> {
        Ok(to_u256(
            self.parameter(overlay, ParameterKey::ScheduleRunFee)?,
        ))
    }

    /// Executes one schedule form for `sender`.
    pub(crate) fn apply_schedule(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        action: &ScheduleAction,
        context: BlockContext,
    ) -> Result<()> {
        require_scheduled_active(&TxKind::Schedule(*action), context)?;
        match *action {
            ScheduleAction::Create {
                payee,
                amount,
                first_due,
                interval,
                runs,
            } => {
                let plan = Plan {
                    payee,
                    amount,
                    first_due,
                    interval,
                    runs,
                };
                self.create_schedule(overlay, sender, nonce, &plan, context.height)
            }
            ScheduleAction::Cancel { schedule_id } => {
                self.cancel_schedule(overlay, sender, &schedule_id)
            }
        }
    }

    fn create_schedule(
        &self,
        overlay: &mut Overlay,
        payer: &Address,
        nonce: u64,
        plan: &Plan,
        height: u64,
    ) -> Result<()> {
        plan.validate(height)?;
        let bound = U256::one() << UNIT_BOUND_BITS;
        let unit = plan
            .amount
            .checked_add(self.run_fee(overlay)?)
            .filter(|unit| *unit < bound)
            .ok_or_else(|| refused("amount + RUN_FEE must be below 2^249"))?;
        // Cannot fail: unit < 2^249 and runs < 2^7 (the const assert above).
        let escrow = unit
            .checked_mul(U256::from(plan.runs))
            .ok_or_else(|| refused("the escrow overflows U256"))?;
        let id = schedule_id(payer, nonce);
        if self.record(overlay, &record_key(&id))?.is_some() {
            return Err(refused("the schedule id is taken"));
        }
        // The payer's own refusal if short: a sender's transaction, so it
        // cannot void someone else's block.
        self.debit_native(overlay, payer, escrow)?;
        let record = ScheduleRecord {
            payer: *payer,
            payee: plan.payee,
            amount: plan.amount,
            next_due: plan.first_due,
            interval: plan.interval,
            remaining: plan.runs,
            escrow,
            missed: 0,
        };
        Self::put_record(overlay, record_key(&id), record.encode());
        Self::put_record(overlay, due_key(record.next_due, &id), Vec::new());
        Ok(())
    }

    fn cancel_schedule(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        id: &[u8; 32],
    ) -> Result<()> {
        let bytes = self
            .record(overlay, &record_key(id))?
            .ok_or_else(|| refused("no such schedule"))?;
        let record = ScheduleRecord::decode(&bytes)?;
        if record.payer != *sender {
            return Err(refused("only the payer may cancel a schedule"));
        }
        self.close_schedule(overlay, id, &record)
    }

    /// Deletes both rows of a schedule and returns its escrow to the payer.
    fn close_schedule(
        &self,
        overlay: &mut Overlay,
        id: &[u8; 32],
        record: &ScheduleRecord,
    ) -> Result<()> {
        Self::delete_record(overlay, record_key(id));
        Self::delete_record(overlay, due_key(record.next_due, id));
        self.credit_native(overlay, &record.payer, record.escrow)
    }

    /// The end-of-block step: runs the schedules due at or before this block,
    /// at most [`MAX_DUE_PER_BLOCK`] of them, in `(next_due, id)` order.
    ///
    /// Dark below bit 46's activation, and skipped while the `Scheduled`
    /// breaker holds: due rows then wait, prepaid, and resume in order (S-11).
    pub(crate) fn settle_schedules(
        &self,
        overlay: &mut Overlay,
        context: BlockContext,
    ) -> Result<()> {
        if !context.scheduled_active() {
            return Ok(());
        }
        if self
            .breaker(overlay, Module::Scheduled)?
            .is_some_and(|record| record.holds_at(context.height))
        {
            return Ok(());
        }
        self.run_due(overlay, context.height, MAX_DUE_PER_BLOCK)
    }

    /// [`Self::settle_schedules`] under an explicit `cap`.
    fn run_due(&self, overlay: &mut Overlay, height: u64, cap: usize) -> Result<()> {
        // A transaction can only delete a committed due row (a cancel) or add
        // one above this height (a create), never add a due one; so reading
        // `cap` plus the staged deletions from disk reaches `cap` live due
        // rows whenever that many exist.
        let staged_deletions = overlay
            .records
            .range(DUE_PREFIX.to_vec()..)
            .take_while(|(key, _)| key.starts_with(DUE_PREFIX))
            .filter(|(_, value)| value.is_none())
            .count();
        let window = cap.saturating_add(staged_deletions);
        let run_fee = self.run_fee(overlay)?;
        let mut taken = 0usize;
        for (key, _) in self.scan_prefix_limit(DUE_PREFIX, window)? {
            if taken >= cap {
                break;
            }
            let (next_due, id) = parse_due_key(&key)?;
            if next_due > height {
                break;
            }
            if matches!(overlay.records.get(&key), Some(None)) {
                continue;
            }
            self.run_one(overlay, &id, next_due, run_fee)?;
            taken += 1;
        }
        Ok(())
    }

    /// One due run: pay or miss, then close or reschedule.
    fn run_one(
        &self,
        overlay: &mut Overlay,
        id: &[u8; 32],
        next_due: u64,
        run_fee: U256,
    ) -> Result<()> {
        let bytes = self
            .record(overlay, &record_key(id))?
            .ok_or_else(|| corrupt("a due row without its schedule"))?;
        let mut record = ScheduleRecord::decode(&bytes)?;
        if record.next_due != next_due || record.remaining == 0 {
            return Err(corrupt("a due row that disagrees with its schedule"));
        }
        match record.amount.checked_add(run_fee) {
            Some(unit) if record.escrow >= unit => {
                self.credit_native(overlay, &record.payee, record.amount)?;
                self.credit_native(overlay, &FEE_COLLECTOR, run_fee)?;
                record.escrow = record
                    .escrow
                    .checked_sub(unit)
                    .ok_or_else(|| corrupt("escrow below a checked run"))?;
                record.remaining -= 1;
                record.missed = 0;
            }
            // Short escrow (RUN_FEE raised by governance, S-8/S-9): a counted
            // no-op, never a block failure.
            _ => record.missed = record.missed.saturating_add(1),
        }
        let later = next_due.checked_add(record.interval);
        match later {
            Some(later) if record.remaining > 0 && record.missed < MAX_MISSED => {
                Self::delete_record(overlay, due_key(next_due, id));
                record.next_due = later;
                Self::put_record(overlay, record_key(id), record.encode());
                Self::put_record(overlay, due_key(later, id), Vec::new());
                Ok(())
            }
            // Done, missed out, or (only after misses) past u64: close.
            _ => self.close_schedule(overlay, id, &record),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_record_is_150_bytes_and_round_trips() {
        let record = ScheduleRecord {
            payer: [1; 32],
            payee: [2; 32],
            amount: U256::MAX,
            next_due: 7,
            interval: MIN_INTERVAL,
            remaining: MAX_RUNS,
            escrow: U256::one(),
            missed: 2,
        };
        let bytes = record.encode();
        assert_eq!(bytes.len(), 150);
        assert_eq!(ScheduleRecord::decode(&bytes).unwrap(), record);
        assert!(ScheduleRecord::decode(&bytes[..149]).is_err());
        let mut wrong = bytes;
        wrong[0] = 0x02;
        assert!(ScheduleRecord::decode(&wrong).is_err());
    }

    #[test]
    fn due_keys_sort_by_height_then_id() {
        let early = due_key(5, &[0xFF; 32]);
        let late = due_key(256, &[0x00; 32]);
        assert!(early < late, "big-endian height leads the key");
        assert_eq!(parse_due_key(&late).unwrap(), (256, [0; 32]));
        assert!(parse_due_key(&late[..late.len() - 1]).is_err());
    }

    #[test]
    fn the_id_binds_payer_and_nonce() {
        assert_ne!(schedule_id(&[1; 32], 0), schedule_id(&[1; 32], 1));
        assert_ne!(schedule_id(&[1; 32], 0), schedule_id(&[2; 32], 0));
    }
}
