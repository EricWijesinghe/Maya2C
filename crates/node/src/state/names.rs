//! Names with commit-reveal (ADR-135 P-3, feature bit 43; ADR-105 rules).
//!
//! ## State (invariant 25: one prefix, one layer)
//!
//! ```text
//! nm:n: ‖ blake3(label)(32)  → 0x01 ‖ len(1) ‖ label ‖ owner ‖ target ‖ registered_at(8 LE) ‖ expires_at(8 LE) ‖ stealth_flag(1)
//! nm:r: ‖ address(32)        → blake3(label)(32)      reverse record, written by the address's own SetReverse
//! nm:c: ‖ commitment(32)     → height(8 LE) ‖ committer(32)
//! commitment = blake3::derive_key("maya2c name commit v1", label ‖ sender ‖ salt)
//! ```
//!
//! The commitment's last 64 input bytes are fixed width (sender, salt), so the
//! variable-length label cannot shift into them: the encoding is unambiguous.
//!
//! ## Money math
//!
//! One `checked_mul` of a governed fee by `periods` (at most 10, so the product
//! fits whenever the fee is below 2^252, which the parameter bounds enforce),
//! then one `checked_add` of the expiry. Every fee is burned to `FEE_SINK`.
//!
//! ## Not in this change
//!
//! Stealth meta-addresses (the record's `stealth_flag` is always 0) and the
//! governance release of the reserved label (a compiled refusal stands in).

use maya_governance::params::ParameterKey;
use maya_stateless_core::amount::to_u256;
use primitive_types::U256;

use crate::core::TxKind;
use crate::core::name_payload::{NameAction, NameUpdate, validate_label};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::shielded::FEE_SINK;

/// Prefix of every name record. One committed layer (`StateLayer::Names`).
pub const NAMES_PREFIX: &[u8] = b"nm:";
const NAME_PREFIX: &[u8] = b"nm:n:";
const REVERSE_PREFIX: &[u8] = b"nm:r:";
const COMMIT_PREFIX: &[u8] = b"nm:c:";

const COMMIT_DOMAIN: &str = "maya2c name commit v1";
/// Blocks a commitment must age before its reveal (threat model M-1, M-2).
pub const MIN_COMMIT_AGE: u64 = 60;
/// Blocks after which a commitment can no longer be revealed.
pub const MAX_COMMIT_AGE: u64 = 86_400;
/// Blocks in one year at one block per second (ADR-135 O-1).
pub const YEAR_BLOCKS: u64 = 31_536_000;
/// Blocks after expiry during which only the owner may renew.
pub const GRACE_BLOCKS: u64 = 7_776_000;
/// Most years one registration or renewal may buy.
pub const MAX_PERIODS_PER_CALL: u8 = 10;
/// Most years a name may be held ahead of the block that renews it.
pub const MAX_HOLD_BLOCKS: u64 = 10 * YEAR_BLOCKS;
/// Labels the protocol holds. Compiled refusal until governance releases them
/// (ADR-135 O-3); `maya2c` is the brand (ADR, brand-name-one-word).
pub const RESERVED_LABELS: &[&[u8]] = &[b"maya2c"];
/// Record version, the first byte of a name record.
const RECORD_VERSION: u8 = 0x01;

/// A registered name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRecord {
    /// The label, ASCII.
    pub label: Vec<u8>,
    /// The owner, who may update and transfer.
    pub owner: Address,
    /// The address the name points at.
    pub target: Address,
    /// Height of the registration.
    pub registered_at: u64,
    /// Height at which the name lapses into its grace period.
    pub expires_at: u64,
}

impl NameRecord {
    /// The record bytes (stealth flag always 0 in this change).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![
            RECORD_VERSION,
            u8::try_from(self.label.len()).unwrap_or(u8::MAX),
        ];
        out.extend_from_slice(&self.label);
        out.extend_from_slice(&self.owner);
        out.extend_from_slice(&self.target);
        out.extend_from_slice(&self.registered_at.to_le_bytes());
        out.extend_from_slice(&self.expires_at.to_le_bytes());
        out.push(0);
        out
    }

    /// Reads a record written by [`Self::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a wrong version, length or a stealth flag this build does not read.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let bad = || NodeError::Decode("name record".into());
        let (&version, rest) = bytes.split_first().ok_or_else(bad)?;
        let (&len, rest) = rest.split_first().ok_or_else(bad)?;
        let len = usize::from(len);
        // 81 fixed bytes: owner, target, two heights, and the stealth flag.
        if version != RECORD_VERSION || rest.len() != len + 81 || rest[rest.len() - 1] != 0 {
            return Err(bad());
        }
        let (label, fixed) = rest.split_at(len);
        Ok(Self {
            label: label.to_vec(),
            owner: array(&fixed[..32]),
            target: array(&fixed[32..64]),
            registered_at: u64_le(&fixed[64..72]),
            expires_at: u64_le(&fixed[72..80]),
        })
    }
}

fn array(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    out
}

fn u64_le(bytes: &[u8]) -> u64 {
    let mut out = [0u8; 8];
    out.copy_from_slice(bytes);
    u64::from_le_bytes(out)
}

/// The commitment to `label` by `sender` with `salt` (ADR-135 P-3).
#[must_use]
pub fn name_commitment(label: &[u8], sender: &Address, salt: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(COMMIT_DOMAIN);
    hasher.update(label);
    hasher.update(sender);
    hasher.update(salt);
    *hasher.finalize().as_bytes()
}

/// Whether bit 43 is active in this block. Admission calls the same check.
///
/// # Errors
///
/// [`NodeError::NameInactive`] for tags 63-66 below the activation height.
pub fn require_names_active(kind: &TxKind, context: BlockContext) -> Result<()> {
    if matches!(kind, TxKind::Name(_)) && !context.names_active() {
        return Err(NodeError::NameInactive {
            height: context.height,
            activation: context.names_activation,
        });
    }
    Ok(())
}

fn refused(reason: &'static str) -> NodeError {
    NodeError::NameRefused { reason }
}

fn name_hash(label: &[u8]) -> [u8; 32] {
    *blake3::hash(label).as_bytes()
}

fn name_key(label: &[u8]) -> Vec<u8> {
    [NAME_PREFIX, &name_hash(label)[..]].concat()
}

fn reverse_key(address: &Address) -> Vec<u8> {
    [REVERSE_PREFIX, &address[..]].concat()
}

fn commit_key(commitment: &[u8; 32]) -> Vec<u8> {
    [COMMIT_PREFIX, &commitment[..]].concat()
}

/// The governed fee for a label of `len` bytes: 3, 4, or 5 and longer.
const fn base_fee_key(len: usize) -> ParameterKey {
    match len {
        3 => ParameterKey::NameBaseFee3,
        4 => ParameterKey::NameBaseFee4,
        _ => ParameterKey::NameBaseFee5,
    }
}

fn require_periods(periods: u8) -> Result<()> {
    if (1..=MAX_PERIODS_PER_CALL).contains(&periods) {
        Ok(())
    } else {
        Err(refused("periods are 1 to 10"))
    }
}

impl StateDB {
    /// A registered name, or `None`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure; a decode error for a corrupt record.
    pub fn get_name(&self, label: &[u8]) -> Result<Option<NameRecord>> {
        self.raw_get(&name_key(label))?
            .map(|bytes| NameRecord::decode(&bytes))
            .transpose()
    }

    /// The label hash an address reverse-resolves to, or `None`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure.
    pub fn reverse_name(&self, address: &Address) -> Result<Option<[u8; 32]>> {
        Ok(self
            .raw_get(&reverse_key(address))?
            .map(|bytes| array(&bytes)))
    }

    /// The height a commitment was made at, or `None`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure.
    pub fn commitment_height(&self, commitment: &[u8; 32]) -> Result<Option<u64>> {
        Ok(self
            .raw_get(&commit_key(commitment))?
            .map(|bytes| u64_le(&bytes[..8])))
    }

    /// Executes one name form for `sender`.
    pub(crate) fn apply_name(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        action: &NameAction,
        context: BlockContext,
    ) -> Result<()> {
        require_names_active(&TxKind::Name(action.clone()), context)?;
        match action {
            NameAction::Commit { commitment } => {
                self.commit_name(overlay, sender, commitment, context)
            }
            NameAction::Register {
                label,
                salt,
                periods,
                target,
            } => self.register_name(overlay, sender, label, salt, *periods, target, context),
            NameAction::Renew { label, periods } => {
                self.renew_name(overlay, sender, label, *periods, context)
            }
            NameAction::Update { label, op } => {
                self.update_name(overlay, sender, label, *op, context)
            }
        }
    }

    fn commit_name(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        commitment: &[u8; 32],
        context: BlockContext,
    ) -> Result<()> {
        if self.record(overlay, &commit_key(commitment))?.is_some() {
            return Err(refused(
                "a commitment already exists; re-committing would reset its age",
            ));
        }
        let mut value = context.height.to_le_bytes().to_vec();
        value.extend_from_slice(sender);
        Self::put_record(overlay, commit_key(commitment), value);
        Ok(())
    }

    fn fee_for(&self, overlay: &Overlay, len: usize, periods: u8) -> Result<U256> {
        let base = to_u256(self.parameter(overlay, base_fee_key(len))?);
        base.checked_mul(U256::from(periods))
            .ok_or_else(|| refused("the name fee overflows U256"))
    }

    fn charge_and_burn(&self, overlay: &mut Overlay, payer: &Address, fee: U256) -> Result<()> {
        if fee.is_zero() {
            return Ok(());
        }
        self.debit_native(overlay, payer, fee)?;
        self.credit_native(overlay, &FEE_SINK, fee)
    }

    /// Clears `address`'s reverse record if it names `hash`.
    fn clear_reverse_if_names(
        &self,
        overlay: &mut Overlay,
        address: &Address,
        hash: &[u8; 32],
    ) -> Result<()> {
        if self.record(overlay, &reverse_key(address))?.as_deref() == Some(&hash[..]) {
            Self::delete_record(overlay, reverse_key(address));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn register_name(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        label: &[u8],
        salt: &[u8; 32],
        periods: u8,
        target: &Address,
        context: BlockContext,
    ) -> Result<()> {
        validate_label(label).map_err(refused)?;
        if RESERVED_LABELS.contains(&label) {
            return Err(refused("the label is reserved"));
        }
        require_periods(periods)?;

        let commitment = name_commitment(label, sender, salt);
        let Some(bytes) = self.record(overlay, &commit_key(&commitment))? else {
            return Err(refused("no commitment for this name by this sender"));
        };
        let committed_at = u64_le(&bytes[..8]);
        if &bytes[8..] != &sender[..] {
            return Err(refused("no commitment for this name by this sender"));
        }
        let age = context.height.saturating_sub(committed_at);
        if age < MIN_COMMIT_AGE {
            return Err(refused("the commitment is too young to reveal"));
        }
        if age > MAX_COMMIT_AGE {
            return Err(refused(
                "the commitment has expired; commit again with a new salt",
            ));
        }

        let hash = name_hash(label);
        if let Some(previous) = self.get_name_overlay(overlay, label)? {
            let lapse = previous.expires_at.saturating_add(GRACE_BLOCKS);
            if context.height <= lapse {
                return Err(refused("the name is taken"));
            }
            self.clear_reverse_if_names(overlay, &previous.owner, &hash)?;
        }

        let fee = self.fee_for(overlay, label.len(), periods)?;
        self.charge_and_burn(overlay, sender, fee)?;
        let span = u64::from(periods)
            .checked_mul(YEAR_BLOCKS)
            .ok_or_else(|| refused("the expiry overflows"))?;
        let expires_at = context
            .height
            .checked_add(span)
            .ok_or_else(|| refused("the expiry overflows"))?;
        let record = NameRecord {
            label: label.to_vec(),
            owner: *sender,
            target: *target,
            registered_at: context.height,
            expires_at,
        };
        Self::put_record(overlay, name_key(label), record.encode());
        Self::delete_record(overlay, commit_key(&commitment));
        Ok(())
    }

    fn get_name_overlay(&self, overlay: &Overlay, label: &[u8]) -> Result<Option<NameRecord>> {
        self.record(overlay, &name_key(label))?
            .map(|bytes| NameRecord::decode(&bytes))
            .transpose()
    }

    fn renew_name(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        label: &[u8],
        periods: u8,
        context: BlockContext,
    ) -> Result<()> {
        require_periods(periods)?;
        let mut record = self
            .get_name_overlay(overlay, label)?
            .ok_or_else(|| refused("no such name"))?;
        if context.height > record.expires_at {
            if context.height > record.expires_at.saturating_add(GRACE_BLOCKS) {
                return Err(refused("the name lapsed past grace; register it again"));
            }
            if record.owner != *sender {
                return Err(refused("only the owner may renew during grace"));
            }
        }
        let fee = self.fee_for(overlay, label.len(), periods)?;
        self.charge_and_burn(overlay, sender, fee)?;
        let extension = u64::from(periods) * YEAR_BLOCKS;
        let ceiling = context.height.saturating_add(MAX_HOLD_BLOCKS);
        record.expires_at = record
            .expires_at
            .checked_add(extension)
            .ok_or_else(|| refused("the expiry overflows"))?
            .min(ceiling);
        Self::put_record(overlay, name_key(label), record.encode());
        Ok(())
    }

    fn update_name(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        label: &[u8],
        op: NameUpdate,
        context: BlockContext,
    ) -> Result<()> {
        let mut record = self
            .get_name_overlay(overlay, label)?
            .ok_or_else(|| refused("no such name"))?;
        if context.height > record.expires_at {
            return Err(refused("the name has expired; renew it first"));
        }
        match op {
            NameUpdate::SetReverse => {
                if record.target != *sender {
                    return Err(refused("the name does not target the sender"));
                }
                Self::put_record(overlay, reverse_key(sender), name_hash(label).to_vec());
                Ok(())
            }
            NameUpdate::SetTarget(target) => {
                require_owner(&record, sender)?;
                record.target = target;
                Self::put_record(overlay, name_key(label), record.encode());
                Ok(())
            }
            NameUpdate::Transfer(new_owner) => {
                require_owner(&record, sender)?;
                self.clear_reverse_if_names(overlay, &record.owner, &name_hash(label))?;
                record.owner = new_owner;
                Self::put_record(overlay, name_key(label), record.encode());
                Ok(())
            }
        }
    }
}

fn require_owner(record: &NameRecord, sender: &Address) -> Result<()> {
    if record.owner == *sender {
        Ok(())
    } else {
        Err(refused("only the owner may change the name"))
    }
}
