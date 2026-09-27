//! Contract deployment and invocation as state transitions.
//!
//! ## Reverting
//!
//! A contract call runs entirely against an owned [`ChainHost`]. Its writes
//! reach the block overlay **only** when execution succeeds. A call that traps
//! or exhausts gas therefore leaves nothing behind: there is no partial write to
//! undo because no write ever left the host.
//!
//! ## Determinism
//!
//! The VM is configured deterministically and its wasmtime version is pinned;
//! see `maya_vm::config`. What this layer must not do is introduce
//! non-determinism of its own, which is why the host is populated from committed
//! state through ordered maps and nothing else.

use std::collections::BTreeMap;

use maya_vm::runtime::Vm;

use crate::core::payload::{ChannelId, ContractCall, ContractDeploy, derive_contract_id};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::contracts::ChainHost;
use crate::state::db::{Overlay, StateDB, code_key, contract_storage_key, contract_storage_prefix};

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
    /// The VM's host must own its data, so the whole key space is loaded before
    /// a call. That bounds how large a contract's storage can usefully become,
    /// and is the price of a host that needs no lifetime games.
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

        // Validate at deploy time. A module that only fails at first call would
        // let a deployer publish garbage that every future caller pays to
        // discover.
        let vm = Vm::new().map_err(|e| NodeError::Vm(e.to_string()))?;
        vm.validate(&deploy.code)
            .map_err(|e| NodeError::Vm(e.to_string()))?;

        overlay.code.insert(contract, deploy.code.clone());
        Ok(contract)
    }

    /// Invokes a contract, staging its writes only on success.
    pub(crate) fn call_contract(
        &self,
        overlay: &mut Overlay,
        caller: &Address,
        call: &ContractCall,
        context: BlockContext,
    ) -> Result<Vec<u8>> {
        let code = match overlay.code.get(&call.contract) {
            Some(code) => code.clone(),
            None => self
                .get_code(&call.contract)?
                .ok_or_else(|| NodeError::UnknownContract(hex::encode(call.contract)))?,
        };

        // Committed storage, plus anything this block already wrote.
        let mut storage = self.contract_storage(&call.contract)?;
        for ((contract, key), value) in &overlay.contract_storage {
            if contract == &call.contract {
                storage.insert(key.clone(), value.clone());
            }
        }

        // Balances the call may observe. The host owns its data, so the set is
        // fixed up front: the caller and the contract's own account. A contract
        // querying anything else sees zero, which is a documented limit of this
        // design rather than an accident.
        let mut balances = BTreeMap::new();
        balances.insert(*caller, self.load_balance(overlay, caller)?);
        balances.insert(call.contract, self.load_balance(overlay, &call.contract)?);

        // The oracle view. Every feed is loaded, not just the ones this call
        // turns out to read: which feeds a contract reads is a function of its
        // input, and a host that fetched them lazily would be a host whose cost
        // depended on control flow the gas meter cannot see.
        //
        // The randomness is the *previous* block's, because the accumulator
        // folds after execution. That is what makes it unpredictable to the
        // transactions in this block rather than merely unknown to them.
        let host = ChainHost::new(context.height)
            .with_caller(*caller)
            .with_storage(storage)
            .with_balances(balances)
            .with_oracle(
                self.beacon_through(overlay)?.map(|state| state.value),
                self.feeds_through(overlay)?,
            )
            .with_zkml(context.zkml_active());

        let vm = Vm::new().map_err(|e| NodeError::Vm(e.to_string()))?;
        let execution = vm.execute(&code, call.contract, &call.input, call.gas_limit, host);

        let outcome = execution
            .outcome
            .map_err(|e| NodeError::Vm(e.to_string()))?;

        // Only now do the writes leave the host. Reaching this line means the
        // call completed within its gas.
        let (writes, _events) = execution.state.into_parts();
        for (key, value) in writes {
            overlay.contract_storage.insert((call.contract, key), value);
        }

        Ok(outcome.output)
    }

    /// Reads a balance through the overlay.
    fn load_balance(&self, overlay: &Overlay, address: &Address) -> Result<u64> {
        match overlay.accounts.get(address) {
            Some(account) => Ok(account.balance),
            None => Ok(self.get_account(address)?.balance),
        }
    }
}
