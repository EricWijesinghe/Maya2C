//! What a contract transaction pays beyond its bytes (Eric 2026-11-24).
//!
//! - **GAS-11 (R-3).** Contract gas is priced by the byte fee's own market:
//!   every `vm.gas_per_fee_byte` units of gas cost one `base_fee`. A call
//!   reserves the price of its `gas_limit` before it runs and pays for the
//!   gas it used; the rest of the reservation goes back to the sender. A
//!   deployment pays for its cold price once its shape is known.
//! - **GAS-12 (R-3).** One sender's contract transactions spend at most
//!   `vm.max_sender_block_gas` of a block's contract gas.
//! - **GAS-13 (R-1).** New contract state pays `vm.storage_fee_bytes_per_byte`
//!   transaction bytes' worth of base fee per byte: a new entry its key, its
//!   value and [`STORAGE_ENTRY_OVERHEAD`]; a longer value what it grew by. A
//!   deployment's code is a new entry too.
//!
//! All of it is base fee, so it burns with the bytes' (FEE-7). Only plaintext
//! transactions pay here: a revealed sealed action keeps its envelope's fee
//! and its own gas budget (GAS-2).

use primitive_types::U256;

use crate::core::{Transaction, TxKind};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::db::{CSTATE_PREFIX, Overlay, StateDB};
use crate::state::fees::FEE_COLLECTOR;
use maya_governance::params::ParameterKey;

/// Bytes a new contract-state entry stores besides its key and value: the
/// stored key's prefix and contract id, and its 32-byte leaf in the contracts
/// root. Derived from the layout, not measured.
pub const STORAGE_ENTRY_OVERHEAD: u64 = (CSTATE_PREFIX.len() + 32 + 32) as u64;

/// What the running plaintext contract transaction may still pay, and the
/// prices it pays at. Fixed when its fee is charged, so a call reads one
/// price for its whole run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContractFee {
    /// The block's base fee per byte.
    base_fee: U256,
    /// What the transaction offered the collector above its byte fee.
    left: U256,
    /// What it reserved for gas before it ran (a call: its `gas_limit`).
    reserved: U256,
    /// `vm.gas_per_fee_byte`, at least 1.
    gas_per_fee_byte: u64,
    /// `vm.storage_fee_bytes_per_byte`.
    storage_fee_bytes: u64,
}

impl ContractFee {
    /// The base fee owed for `gas` units of gas and `growth` new bytes.
    pub(crate) fn owed(&self, gas: u64, growth: u64) -> U256 {
        let units = u128::from(gas.div_ceil(self.gas_per_fee_byte))
            + u128::from(growth) * u128::from(self.storage_fee_bytes);
        // Saturating: a fee past 2^256 is one no balance can pay, so it
        // refuses rather than wraps (as `FeeRecord::required` does).
        self.base_fee
            .checked_mul(U256::from(units))
            .unwrap_or(U256::MAX)
    }

    /// What it offered above its byte fee.
    pub(crate) const fn left(&self) -> U256 {
        self.left
    }

    /// What it reserved before it ran.
    pub(crate) const fn owed_reserved(&self) -> U256 {
        self.reserved
    }

    /// Whether `gas` units of gas and `growth` new bytes fit what was offered.
    pub(crate) fn affords(&self, gas: u64, growth: u64) -> bool {
        self.owed(gas, growth) <= self.left
    }
}

/// The gas a contract transaction reserves before it runs (GAS-11): a call's
/// `gas_limit`, nothing for a deployment (priced after its scan).
const fn reserved_gas(kind: &TxKind) -> u64 {
    match kind {
        TxKind::CallContract(call) => call.gas_limit,
        _ => 0,
    }
}

impl StateDB {
    /// The base fee a contract transaction owes beyond its bytes before it
    /// may run (GAS-11), and the prices it runs at. `None` for any other
    /// kind. Read through `overlay`, so a block sees the prices in force.
    ///
    /// # Errors
    ///
    /// Storage failure.
    pub(crate) fn contract_fee_for(
        &self,
        overlay: &Overlay,
        kind: &TxKind,
        base_fee: U256,
        left: U256,
    ) -> Result<Option<ContractFee>> {
        if !matches!(kind, TxKind::CallContract(_) | TxKind::DeployContract(_)) {
            return Ok(None);
        }
        let mut fee = ContractFee {
            base_fee,
            left,
            reserved: U256::zero(),
            gas_per_fee_byte: self
                .parameter_u64(overlay, ParameterKey::VmGasPerFeeByte)?
                .max(1),
            storage_fee_bytes: self
                .parameter_u64(overlay, ParameterKey::VmStorageFeeBytesPerByte)?,
        };
        fee.reserved = fee.owed(reserved_gas(kind), 0);
        Ok(Some(fee))
    }

    /// Mempool admission (GAS-11, GAS-13): a contract transaction must offer
    /// what it would owe at the committed base fee. A call: its bytes and its
    /// `gas_limit`. A deployment, after its shape scan: its bytes, its cold
    /// price and its code as new state. So nothing pooled runs unpaid.
    ///
    /// # Errors
    ///
    /// [`NodeError::FeeTooLow`], [`NodeError::Vm`] for a module the scan
    /// refuses, or a storage failure.
    pub(crate) fn admit_contract_fee(&self, tx: &Transaction) -> Result<()> {
        let Some(fees) = self.committed_fees()? else {
            return Ok(());
        };
        let byte_fee = maya_stateless_core::amount::to_u256(fees.required(tx.to_bytes().len()));
        let offered = collector_outputs(tx);
        let base = maya_stateless_core::amount::to_u256(fees.base_fee);
        let overlay = Overlay::default();
        let Some(fee) =
            self.contract_fee_for(&overlay, &tx.kind, base, offered.saturating_sub(byte_fee))?
        else {
            return Ok(());
        };
        let (gas, growth) = match &tx.kind {
            TxKind::DeployContract(deploy) => (
                maya_vm::gas::cold_call_fuel(&deploy.code)
                    .map_err(|e| NodeError::Vm(e.to_string()))?,
                deploy_growth(deploy.code.len()),
            ),
            kind => (reserved_gas(kind), 0),
        };
        let owed = fee.owed(gas, growth);
        if offered < byte_fee || owed > fee.left {
            return Err(NodeError::FeeTooLow {
                required: byte_fee.saturating_add(owed),
                offered,
            });
        }
        Ok(())
    }

    /// GAS-12: refuses contract work that would take `sender` past
    /// `vm.max_sender_block_gas` in this block. Plaintext transactions only:
    /// revealed sealed actions share a budget of their own (GAS-2).
    ///
    /// # Errors
    ///
    /// [`NodeError::SenderGasExhausted`], or a storage failure.
    pub(crate) fn admit_sender_gas(
        &self,
        overlay: &Overlay,
        sender: &Address,
        needed: u64,
    ) -> Result<()> {
        if overlay.revealing {
            return Ok(());
        }
        let cap = self.parameter_u64(overlay, ParameterKey::VmMaxSenderBlockGas)?;
        let used = overlay.sender_gas.get(sender).copied().unwrap_or(0);
        let remaining = cap.saturating_sub(used);
        if needed > remaining {
            return Err(NodeError::SenderGasExhausted { needed, remaining });
        }
        Ok(())
    }

    /// Settles a plaintext contract transaction that applied (or was
    /// included as failed, GAS-8): counts `spent` gas against its sender
    /// (GAS-12), books the base fee for the gas it used and the state it
    /// grew to burn, and returns what it reserved and did not use to the
    /// sender (GAS-11).
    ///
    /// # Errors
    ///
    /// [`NodeError::FeeTooLow`] if what it owes passed what it offered,
    /// which the checks before staging make unreachable; overflow; storage.
    pub(crate) fn settle_contract_fee(
        &self,
        overlay: &mut Overlay,
        tx: &Transaction,
        sender: &Address,
        spent: u64,
    ) -> Result<()> {
        let growth = std::mem::take(&mut overlay.vm_growth);
        let used = overlay.sender_gas.entry(*sender).or_insert(0);
        *used = used.saturating_add(spent);
        let Some(fee) = overlay.contract_fee.take() else {
            return Ok(());
        };
        // A call pays for no more than it named: its failure gas may pass
        // `gas_limit` by its memory (GAS-2), which the block counts and the
        // sender does not owe.
        let billed = match &tx.kind {
            TxKind::CallContract(call) => spent.min(call.gas_limit),
            _ => spent,
        };
        let owed = fee.owed(billed, growth);
        if owed > fee.left {
            return Err(NodeError::FeeTooLow {
                required: owed,
                offered: fee.left,
            });
        }
        overlay.fee_burn = overlay
            .fee_burn
            .checked_add(owed)
            .ok_or(NodeError::BalanceOverflow)?;
        let refund = fee.reserved.saturating_sub(owed);
        if !refund.is_zero() {
            let mut collector = self.load(overlay, &FEE_COLLECTOR)?;
            collector.balance = collector.balance.checked_sub(refund).ok_or_else(|| {
                NodeError::InvariantViolation("gas refund exceeds what was collected".into())
            })?;
            overlay.accounts.insert(FEE_COLLECTOR, collector);
            let mut account = self.load(overlay, sender)?;
            account.balance = account
                .balance
                .checked_add(refund)
                .ok_or(NodeError::BalanceOverflow)?;
            overlay.accounts.insert(*sender, account);
        }
        Ok(())
    }
}

/// New state a deployment stores: its code, as one entry (GAS-13).
pub(crate) const fn deploy_growth(code_len: usize) -> u64 {
    (code_len as u64).saturating_add(STORAGE_ENTRY_OVERHEAD)
}

/// New state one write stores, given what the key held before (GAS-13).
pub(crate) fn write_growth(key: &[u8], value: &[u8], before: Option<usize>) -> u64 {
    match before {
        None => (key.len() as u64)
            .saturating_add(value.len() as u64)
            .saturating_add(STORAGE_ENTRY_OVERHEAD),
        Some(old) => value.len().saturating_sub(old) as u64,
    }
}

/// Everything `tx` pays the fee collector.
pub(crate) fn collector_outputs(tx: &Transaction) -> U256 {
    // Saturating: the outputs already passed the transaction's checked
    // output total, so a sum past 2^256 cannot reach here.
    tx.outputs
        .iter()
        .filter(|o| o.recipient == FEE_COLLECTOR)
        .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fee(left: u64) -> ContractFee {
        ContractFee {
            base_fee: U256::from(3u64),
            left: U256::from(left),
            reserved: U256::zero(),
            gas_per_fee_byte: 47,
            storage_fee_bytes: 40,
        }
    }

    #[test]
    fn gas_rounds_up_to_a_whole_fee_byte_and_state_pays_per_byte() {
        let f = fee(u64::MAX);
        assert_eq!(f.owed(0, 0), U256::zero());
        assert_eq!(f.owed(1, 0), U256::from(3u64), "one unit is a whole byte");
        assert_eq!(f.owed(47, 0), U256::from(3u64));
        assert_eq!(f.owed(48, 0), U256::from(6u64));
        assert_eq!(f.owed(0, 2), U256::from(3u64 * 40 * 2));
    }

    #[test]
    fn a_fee_past_two_to_the_256_refuses_rather_than_wraps() {
        let f = ContractFee {
            base_fee: U256::MAX,
            ..fee(u64::MAX)
        };
        assert_eq!(f.owed(u64::MAX, u64::MAX), U256::MAX);
        assert!(!f.affords(u64::MAX, 0));
    }

    #[test]
    fn a_sender_past_its_share_of_the_block_is_refused_and_others_are_not() {
        // GAS-12 (R-3): one sender's contract gas in a block is capped.
        let dir = tempfile::TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        let cap = ParameterKey::VmMaxSenderBlockGas
            .bounds()
            .default
            .saturating_to_u64();
        let mut overlay = Overlay::new();
        overlay.sender_gas.insert([1; 32], cap - 10);
        assert!(db.admit_sender_gas(&overlay, &[1; 32], 10).is_ok());
        assert!(matches!(
            db.admit_sender_gas(&overlay, &[1; 32], 11),
            Err(NodeError::SenderGasExhausted {
                needed: 11,
                remaining: 10
            })
        ));
        assert!(
            db.admit_sender_gas(&overlay, &[2; 32], cap).is_ok(),
            "another sender"
        );
        overlay.revealing = true;
        assert!(
            db.admit_sender_gas(&overlay, &[1; 32], 11).is_ok(),
            "revealed sealed actions have their own budget"
        );
    }

    #[test]
    fn admission_refuses_a_call_no_senders_share_of_a_block_could_hold() {
        // PR #701 review: governance may set `vm.max_sender_block_gas` below
        // `vm.max_tx_gas`. A call between the two passed GAS-1 at admission,
        // then GAS-12 refused it in every block before any VM work, so it was
        // never paid for and sat in every pool until it expired.
        use crate::core::payload::ContractCall;
        let dir = tempfile::TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        let sender_cap = ParameterKey::VmMaxSenderBlockGas
            .bounds()
            .min
            .saturating_to_u64();
        let tx_cap = ParameterKey::VmMaxTxGas
            .bounds()
            .default
            .saturating_to_u64();
        assert!(sender_cap < tx_cap);
        let table = crate::governance::ParameterTable::new().with(
            ParameterKey::VmMaxSenderBlockGas,
            maya_dex::Amount::from_u64(sender_cap),
        );
        db.raw_put_for_test(crate::governance::PARAMETERS_KEY, &table.encode())
            .unwrap();
        let call = |gas_limit| {
            TxKind::CallContract(ContractCall {
                contract: [0; 32],
                input: Vec::new(),
                gas_limit,
            })
        };
        assert!(db.admit_vm_gas(&call(sender_cap)).is_ok());
        assert!(matches!(
            db.admit_vm_gas(&call(sender_cap + 1)),
            Err(NodeError::GasAboveCap { cap, .. }) if cap == sender_cap
        ));
    }

    #[test]
    fn only_new_bytes_grow_state() {
        assert_eq!(write_growth(b"k", b"vv", None), 3 + STORAGE_ENTRY_OVERHEAD);
        assert_eq!(write_growth(b"k", b"vvvv", Some(2)), 2);
        assert_eq!(
            write_growth(b"k", b"v", Some(4)),
            0,
            "a shrink is not refunded"
        );
        assert_eq!(deploy_growth(100), 100 + STORAGE_ENTRY_OVERHEAD);
    }
}
