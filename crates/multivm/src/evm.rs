//! revm as a hosted engine.
//!
//! One `CacheDB` holds the EVM's view of state for the engine's lifetime:
//! balances are funded from Maya accounts through [`crate::state::evm_address`],
//! contracts keep their EVM storage, and every call reports EVM gas and its
//! Maya fuel price.

use revm::context::TxEnv;
use revm::context::result::{ExecutionResult, Output};
use revm::database::{CacheDB, EmptyDB};
use revm::primitives::{Address, Bytes, TxKind, U256};
use revm::state::AccountInfo;
use revm::{Context, DatabaseRef, ExecuteCommitEvm, MainBuilder, MainContext};

use crate::MultiVmError;
use crate::gas::evm_gas_to_fuel;
use crate::state::{MayaAddress, evm_address};

/// What one EVM call or deployment did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmOutcome {
    /// Return data, or the deployed contract's address for a deployment.
    pub output: Vec<u8>,
    /// EVM gas used.
    pub gas_used: u64,
    /// The same, in Maya VM fuel.
    pub fuel: u64,
}

/// A hosted EVM.
#[derive(Debug, Default)]
pub struct EvmEngine {
    db: CacheDB<EmptyDB>,
}

fn engine_err(e: impl std::fmt::Debug) -> MultiVmError {
    MultiVmError::Engine {
        engine: "revm",
        message: format!("{e:?}"),
    }
}

impl EvmEngine {
    /// An empty EVM world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mirrors a Maya balance into the EVM, one base unit per wei.
    pub fn fund(&mut self, maya: &MayaAddress, balance: u64) {
        let address = Address::from(evm_address(maya));
        let mut info = self
            .db
            .basic_ref(address)
            .ok()
            .flatten()
            .unwrap_or_default();
        info.balance = U256::from(balance);
        self.db.insert_account_info(address, AccountInfo { ..info });
    }

    /// A Maya account's balance as the EVM sees it.
    #[must_use]
    pub fn balance(&self, maya: &MayaAddress) -> U256 {
        self.balance_of(Address::from(evm_address(maya)))
    }

    /// Any EVM address's balance.
    #[must_use]
    pub fn balance_of(&self, address: Address) -> U256 {
        self.db
            .basic_ref(address)
            .ok()
            .flatten()
            .map_or(U256::ZERO, |i| i.balance)
    }

    fn transact(
        &mut self,
        sender: &MayaAddress,
        kind: TxKind,
        data: Vec<u8>,
        value: u64,
        gas_limit: u64,
    ) -> Result<EvmOutcome, MultiVmError> {
        let caller = Address::from(evm_address(sender));
        let nonce = self
            .db
            .basic_ref(caller)
            .ok()
            .flatten()
            .map_or(0, |i| i.nonce);
        let tx = TxEnv::builder()
            .caller(caller)
            .kind(kind)
            .data(Bytes::from(data))
            .value(U256::from(value))
            .gas_limit(gas_limit)
            .gas_price(0)
            .nonce(nonce)
            .build()
            .map_err(engine_err)?;
        let db = std::mem::take(&mut self.db);
        let mut evm = Context::mainnet().with_db(db).build_mainnet();
        let result = evm.transact_commit(tx);
        self.db = std::mem::take(&mut evm.ctx.journaled_state.database);
        match result.map_err(engine_err)? {
            ExecutionResult::Success { gas, output, .. } => Ok(EvmOutcome {
                output: match output {
                    Output::Call(bytes) => bytes.to_vec(),
                    Output::Create(_, address) => address.map(|a| a.to_vec()).unwrap_or_default(),
                },
                gas_used: gas.total_gas_spent(),
                fuel: evm_gas_to_fuel(gas.total_gas_spent()),
            }),
            other => Err(MultiVmError::Failed {
                engine: "revm",
                message: format!("{other:?}"),
            }),
        }
    }

    /// Deploys `init_code` from `sender`; the outcome's output is the
    /// 20-byte contract address.
    ///
    /// # Errors
    ///
    /// The engine refused the transaction, or the constructor reverted.
    pub fn deploy(
        &mut self,
        sender: &MayaAddress,
        init_code: Vec<u8>,
        gas_limit: u64,
    ) -> Result<EvmOutcome, MultiVmError> {
        self.transact(sender, TxKind::Create, init_code, 0, gas_limit)
    }

    /// Calls `contract` from `sender` with `data` and `value`.
    ///
    /// # Errors
    ///
    /// The engine refused the transaction, or the call reverted or halted.
    pub fn call(
        &mut self,
        sender: &MayaAddress,
        contract: [u8; 20],
        data: Vec<u8>,
        value: u64,
        gas_limit: u64,
    ) -> Result<EvmOutcome, MultiVmError> {
        self.transact(
            sender,
            TxKind::Call(Address::from(contract)),
            data,
            value,
            gas_limit,
        )
    }
}
