//! Contract deployment and invocation as state transitions.
//!
//! ## Reverting
//!
//! A contract call runs entirely against an owned [`ChainHost`]. Its writes
//! reach the block overlay **only** when execution succeeds. A call that traps
//! or exhausts gas therefore leaves nothing behind: there is no partial write to
//! undo because no write ever left the host.
//!
//! ## Gas caps (ADR-142)
//!
//! A call's `gas_limit` is capped at `min(vm.max_tx_gas, vm.max_block_gas)`,
//! and the block's contract work at `vm.max_block_gas`: calls add the gas they
//! used, deployments the compile they cost. Both are refused before any work
//! (GAS-1, GAS-2 in `spec/10-vm-gas.md`). The first call to a contract in a
//! block compiles it and pays for that; later calls in the block reuse the
//! module held in the overlay and pay only to instantiate it (GAS-3).
//!
//! ## Determinism
//!
//! The VM is configured deterministically and its wasmtime version is pinned;
//! see `maya_vm::config`. What this layer must not do is introduce
//! non-determinism of its own, which is why the host is populated from committed
//! state through ordered maps and nothing else.

use std::collections::BTreeMap;

use maya_governance::params::ParameterKey;
use maya_vm::gas;

use crate::core::TxKind;
use crate::core::payload::{ChannelId, ContractCall, ContractDeploy, derive_contract_id};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::contracts::ChainHost;
use crate::state::db::{Overlay, StateDB, code_key, contract_storage_key, contract_storage_prefix};

/// GAS-1 and GAS-2: whether a call naming `gas_limit` may run in a block that
/// has spent `block_used` of `block_cap`.
///
/// # Errors
///
/// [`NodeError::GasAboveCap`] above `min(tx_cap, block_cap)`, then
/// [`NodeError::BlockGasExhausted`] above what the block has left.
pub fn admit_call_gas(gas_limit: u64, tx_cap: u64, block_cap: u64, block_used: u64) -> Result<()> {
    let cap = tx_cap.min(block_cap);
    if gas_limit > cap {
        return Err(NodeError::GasAboveCap {
            limit: gas_limit,
            cap,
        });
    }
    admit_block_gas(gas_limit, block_cap, block_used)
}

/// GAS-2 for a deployment of `code_len` bytes.
///
/// # Errors
///
/// [`NodeError::GasAboveCap`] when the compile alone is above the block cap,
/// then [`NodeError::BlockGasExhausted`] above what the block has left.
pub fn admit_deploy_gas(code_len: usize, block_cap: u64, block_used: u64) -> Result<()> {
    let needed = gas::deploy_fuel(code_len);
    if needed > block_cap {
        return Err(NodeError::GasAboveCap {
            limit: needed,
            cap: block_cap,
        });
    }
    admit_block_gas(needed, block_cap, block_used)
}

fn admit_block_gas(needed: u64, block_cap: u64, block_used: u64) -> Result<()> {
    let remaining = block_cap.saturating_sub(block_used);
    if needed > remaining {
        return Err(NodeError::BlockGasExhausted { needed, remaining });
    }
    Ok(())
}

impl StateDB {
    /// Reads deployed contract code.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn get_code(&self, contract: &ChannelId) -> Result<Option<Vec<u8>>> {
        self.raw_get(&code_key(contract))
    }

    /// Reads one contract storage key from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn get_contract_storage(
        &self,
        contract: &ChannelId,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        self.raw_get(&contract_storage_key(contract, key))
    }

    /// Reads a contract's entire storage.
    ///
    /// For explorers, RPC and tests. A call no longer loads this (ADR-142):
    /// its host reads one key at a time, see [`ChainHost`].
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure.
    pub fn contract_storage(&self, contract: &ChannelId) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
        let prefix = contract_storage_prefix(contract);
        let mut out = BTreeMap::new();

        for (key, value) in self.scan_prefix(&prefix)? {
            out.insert(key[prefix.len()..].to_vec(), value);
        }

        Ok(out)
    }

    /// Stores a contract module, returning its derived address.
    pub(crate) fn deploy_contract(
        &self,
        overlay: &mut Overlay,
        deployer: &Address,
        nonce: u64,
        deploy: &ContractDeploy,
    ) -> Result<ChannelId> {
        let contract = derive_contract_id(deployer, nonce, &deploy.code);

        if overlay.code.contains_key(&contract) || self.get_code(&contract)?.is_some() {
            return Err(NodeError::ContractExists(hex::encode(contract)));
        }

        // Validating compiles the module, so it counts against the block's
        // contract gas (GAS-2), and is refused before the compile when it
        // does not fit.
        let block_cap = self.parameter_u64(overlay, ParameterKey::VmMaxBlockGas)?;
        admit_deploy_gas(deploy.code.len(), block_cap, overlay.vm_gas)?;

        // Validate at deploy time. A module that only fails at first call would
        // let a deployer publish garbage that every future caller pays to
        // discover.
        self.vm()?
            .validate(&deploy.code)
            .map_err(|e| NodeError::Vm(e.to_string()))?;

        overlay.vm_gas = overlay
            .vm_gas
            .saturating_add(gas::deploy_fuel(deploy.code.len()));
        overlay.code.insert(contract, deploy.code.clone());
        Ok(contract)
    }

    /// Mempool admission for contract gas (GAS-1, GAS-2), against committed
    /// parameters: a call naming more than one transaction may, or a
    /// deployment no block could hold, can never be included.
    ///
    /// # Errors
    ///
    /// [`NodeError::GasAboveCap`], or [`NodeError::Storage`] on a read failure.
    pub(crate) fn admit_vm_gas(&self, kind: &TxKind) -> Result<()> {
        let table = self.parameters()?;
        let block_cap = table.get(ParameterKey::VmMaxBlockGas).saturating_to_u64();
        match kind {
            TxKind::CallContract(call) => {
                let tx_cap = table.get(ParameterKey::VmMaxTxGas).saturating_to_u64();
                admit_call_gas(call.gas_limit, tx_cap, block_cap, 0)
            }
            TxKind::DeployContract(deploy) => admit_deploy_gas(deploy.code.len(), block_cap, 0),
            _ => Ok(()),
        }
    }

    /// The host a call to `contract` runs against: lazy committed storage
    /// behind this block's earlier writes to it (ADR-142), the caller's and the
    /// contract's balances, and the oracle view.
    fn contract_host(
        &self,
        overlay: &Overlay,
        caller: &Address,
        contract: &ChannelId,
        context: BlockContext,
    ) -> Result<ChainHost> {
        // This block's writes to this contract only: a range of the ordered
        // overlay, not a pass over every contract's writes.
        let staged: BTreeMap<Vec<u8>, Vec<u8>> = overlay
            .contract_storage
            .range((*contract, Vec::new())..)
            .take_while(|((owner, _), _)| owner == contract)
            .map(|((_, key), value)| (key.clone(), value.clone()))
            .collect();

        // Balances the call may observe. The host owns its data, so the set is
        // fixed up front: the caller and the contract's own account. A contract
        // querying anything else sees zero, which is a documented limit of this
        // design rather than an accident.
        let mut balances = BTreeMap::new();
        balances.insert(*caller, self.load_balance(overlay, caller)?);
        balances.insert(*contract, self.load_balance(overlay, contract)?);

        // The oracle view. Every feed is loaded, not just the ones this call
        // turns out to read: which feeds a contract reads is a function of its
        // input, and a host that fetched them lazily would be a host whose cost
        // depended on control flow the gas meter cannot see.
        //
        // The randomness is the *previous* block's, because the accumulator
        // folds after execution. That is what makes it unpredictable to the
        // transactions in this block rather than merely unknown to them.
        Ok(ChainHost::new(context.height)
            .with_caller(*caller)
            .with_storage(staged)
            .with_committed(self.shared_db(), contract_storage_prefix(contract))
            .with_balances(balances)
            .with_oracle(
                self.beacon_through(overlay)?.map(|state| state.value),
                self.feeds_through(overlay)?,
            )
            .with_zkml(context.zkml_active()))
    }

    /// Invokes a contract, staging its writes only on success.
    pub(crate) fn call_contract(
        &self,
        overlay: &mut Overlay,
        caller: &Address,
        call: &ContractCall,
        context: BlockContext,
    ) -> Result<Vec<u8>> {
        // The caps first, before the code is read or compiled (GAS-1, GAS-2).
        let tx_cap = self.parameter_u64(overlay, ParameterKey::VmMaxTxGas)?;
        let block_cap = self.parameter_u64(overlay, ParameterKey::VmMaxBlockGas)?;
        admit_call_gas(call.gas_limit, tx_cap, block_cap, overlay.vm_gas)?;

        let code = match overlay.code.get(&call.contract) {
            Some(code) => code.clone(),
            None => self
                .get_code(&call.contract)?
                .ok_or_else(|| NodeError::UnknownContract(hex::encode(call.contract)))?,
        };

        // Warm if this block already compiled it (GAS-3). A call that cannot
        // pay its upfront price is refused here, before the compile; the VM
        // applies the same price again and would refuse it too.
        let warm = overlay.vm_modules.get(&call.contract).cloned();
        let upfront = gas::call_fuel(code.len(), warm.is_some());
        if call.gas_limit < upfront {
            return Err(NodeError::Vm(
                maya_vm::VmError::OutOfGas {
                    limit: call.gas_limit,
                }
                .to_string(),
            ));
        }
        let vm = self.vm()?;
        let (compiled, is_warm) = match warm {
            Some(compiled) => (compiled, true),
            None => (
                vm.compile(&code)
                    .map_err(|e| NodeError::Vm(e.to_string()))?,
                false,
            ),
        };

        let host = self.contract_host(overlay, caller, &call.contract, context)?;
        let execution = vm.execute_compiled(
            &compiled,
            is_warm,
            call.contract,
            &call.input,
            call.gas_limit,
            host,
        );

        // A database fault during a lazy read is this node's, not the call's:
        // it fails as `Storage`, whatever the contract made of the miss.
        if let Some(error) = execution.state.read_error() {
            return Err(NodeError::Storage(error.to_string()));
        }
        let outcome = match execution.outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                // The work ran, up to its limit. A caller that keeps going
                // after a failed call (a revealed sealed action) keeps this
                // spent; for a plain transaction the block fails anyway.
                overlay.vm_gas = overlay.vm_gas.saturating_add(call.gas_limit);
                return Err(NodeError::Vm(error.to_string()));
            }
        };

        // Only now do the writes leave the host. Reaching this line means the
        // call completed within its gas, which `admit_call_gas` bounded by what
        // the block had left.
        overlay.vm_gas = overlay.vm_gas.saturating_add(outcome.gas_used);
        overlay.vm_modules.insert(call.contract, compiled);
        let (writes, _events) = execution.state.into_parts();
        for (key, value) in writes {
            overlay.contract_storage.insert((call.contract, key), value);
        }

        Ok(outcome.output)
    }

    /// Reads a balance through the overlay, 256-bit (ADR-046): contracts see
    /// the exact value through `get_balance_u256`.
    fn load_balance(&self, overlay: &Overlay, address: &Address) -> Result<maya_vm::Amount> {
        let balance = match overlay.accounts.get(address) {
            Some(account) => account.balance,
            None => self.get_account(address)?.balance,
        };
        Ok(maya_stateless_core::amount::to_amount(balance))
    }
}

#[cfg(test)]
#[path = "vm_exec_tests.rs"]
mod tests;
