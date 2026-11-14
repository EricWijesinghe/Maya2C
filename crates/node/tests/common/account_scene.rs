//! Fixtures shared by the smart-account test files (ADR-135 P-5): a chain
//! with both activation heights set, an opted-in owner and one session key.
//! Included with `#[path]` by `account_policy_tests.rs` and
//! `v4_redteam_accounts.rs`, so each uses only part of it.

#![allow(dead_code, unused_imports, clippy::unwrap_used, clippy::expect_used)]

pub use custom_l1_node::core::account_payload::{AccountAction, SessionScope};
pub use custom_l1_node::core::account_tx::signer_policy;
pub use custom_l1_node::core::multisig_tx::MultisigAuth;
pub use custom_l1_node::core::{Block, BlockHeader, ChainTag, Transaction, TxKind, TxOutput};
pub use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
pub use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
pub use custom_l1_node::network::mempool::Mempool;
pub use custom_l1_node::state::account_policy::{MAX_SESSION_COST, WINDOW_BLOCKS};
pub use custom_l1_node::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
pub use custom_l1_node::state::upgrades::version_key;
pub use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
pub use custom_l1_node::upgrade::FeatureSet;
pub use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
pub use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa65, SignatureSuite, SlhDsaSha2_128s, SuiteId,
};
pub use maya_dex::Amount;
pub use primitive_types::U256;
pub use std::sync::Arc;
pub use tempfile::TempDir;

pub const CHAIN: ChainTag = ChainTag::from_genesis([42; 32]);
pub const FUNDS: u64 = 1_000_000;
pub const PAYEE: Address = [0xB0; 32];
pub const STRANGER: Address = [0xBE; 32];
pub const CAP: u64 = 100;
pub const EXPIRES: u64 = 50;
pub const OWNER_SEED: u8 = 0x01;

pub type Key65 = <MlDsa65 as SignatureSuite>::SigningKey;

// ---------------------------------------------------------------- fixtures --

pub fn device(seed: u8) -> Key65 {
    MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]))
}

pub fn session_policy(key: &Key65) -> MultisigPolicy {
    let listed = vec![PolicyKey {
        suite: SuiteId::MlDsa65,
        public_key: MlDsa65::public_key(key),
    }];
    MultisigPolicy::new(1, listed).unwrap()
}

pub struct Chain {
    pub db: StateDB,
    pub height: u64,
    pub authority: u64,
    pub policy: u64,
    pub _dir: TempDir,
}

impl Chain {
    pub fn new(funded: &[Address], authority: u64, policy: u64) -> Self {
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        db.bind_chain(CHAIN).unwrap();
        for a in funded {
            let account = Account {
                balance: U256::from(FUNDS),
                nonce: 0,
            };
            db.put_account(a, &account).unwrap();
        }
        Self {
            db,
            height: 0,
            authority,
            policy,
            _dir: dir,
        }
    }

    pub fn context(&self, height: u64) -> BlockContext {
        BlockContext::at_height(height)
            .with_account_authority_activation(self.authority)
            .with_account_policy_activation(self.policy)
    }

    pub fn block(&self, txs: Vec<Transaction>) -> Block {
        let header = BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_790_000_000 + self.height,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        };
        Block::new(header, txs)
    }

    /// One transaction in the next block; `Err` if the block is refused.
    pub fn send(&mut self, tx: Transaction) -> Result<(), String> {
        self.height += 1;
        let block = self.block(vec![tx]);
        self.db
            .apply_block(&block, self.context(self.height))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The next block will be at `height`.
    pub fn next_at(&mut self, height: u64) {
        self.height = height - 1;
    }

    pub fn nonce(&self, a: &Address) -> u64 {
        self.db.get_account(a).unwrap().nonce
    }

    pub fn balance(&self, a: &Address) -> U256 {
        self.db.get_account(a).unwrap().balance
    }
}

pub fn pay_to(recipient: Address, amount: u64) -> Vec<TxOutput> {
    vec![TxOutput {
        amount: U256::from(amount),
        recipient,
    }]
}

/// `outputs` plus a fee collector output of `fee`.
pub fn with_fee(mut outputs: Vec<TxOutput>, fee: u64) -> Vec<TxOutput> {
    outputs.push(TxOutput {
        amount: U256::from(fee),
        recipient: FEE_COLLECTOR,
    });
    outputs
}

/// The required fee of the frame `make` builds: at a base fee of 1 per byte
/// it is the frame's size, which the fixed-width encoding keeps independent of
/// the amounts.
pub fn required_fee(make: impl Fn(u64) -> Transaction) -> u64 {
    u64::try_from(make(1).to_bytes().len()).unwrap()
}

pub fn account(action: AccountAction) -> TxKind {
    TxKind::Account(Box::new(action))
}

pub fn scope(max_amount: u64, expires_at: u64, recipients: Vec<Address>) -> SessionScope {
    SessionScope {
        max_amount: U256::from(max_amount),
        expires_at,
        recipients,
    }
}

/// A v9 frame for `account` under `policy`, with the approvals `sign` makes.
pub fn v9_frame(
    account: Address,
    policy: &MultisigPolicy,
    kind: TxKind,
    outputs: Vec<TxOutput>,
    nonce: u64,
    sign: impl Fn(&[u8]) -> Vec<Approval>,
) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.outputs = outputs;
    tx.multisig = Some(Box::new(
        MultisigAuth::unsigned(policy.clone()).bound_to(account),
    ));
    let approvals = sign(&tx.signing_bytes(&CHAIN));
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy.clone(), approvals)
            .unwrap()
            .bound_to(account),
    ));
    tx
}

/// An owner hybrid key opted in by its first `AddSessionKey`, and one
/// ML-DSA-65 session key allowed to pay `PAYEE` up to `CAP` until `EXPIRES`.
pub struct Scene {
    pub chain: Chain,
    pub owner: HybridSigningKey,
    pub address: Address,
    pub owner_policy: MultisigPolicy,
    pub session: Key65,
    pub session_policy: MultisigPolicy,
}

impl Scene {
    pub fn new() -> Self {
        Self::with_policy_activation(1)
    }

    pub fn with_policy_activation(policy: u64) -> Self {
        let owner = signing_key_from_seed(&[OWNER_SEED; 32]).unwrap();
        let address = owner.address();
        let session = device(0x09);
        let session_policy = session_policy(&session);
        let chain = Chain::new(&[address], 1, policy);
        // The owner's 1-of-1 hybrid policy: what the opt-in records (Rule B).
        let mut probe = Transaction::with_kind(TxKind::Transfer, 0);
        probe.sign(&owner, &CHAIN).unwrap();
        let owner_policy = signer_policy(&probe).unwrap();
        Self {
            chain,
            owner,
            address,
            owner_policy,
            session,
            session_policy,
        }
    }

    /// The owner's first action, a v6 frame that opts the account in.
    pub fn opt_in_with_session(&mut self) -> Result<(), String> {
        let action = AccountAction::AddSessionKey {
            key_hash: self.session_policy.digest(),
            scope: scope(CAP, EXPIRES, vec![PAYEE]),
        };
        let mut tx = Transaction::with_kind(account(action), self.chain.nonce(&self.address));
        tx.sign(&self.owner, &CHAIN).unwrap();
        self.chain.send(tx)
    }

    pub fn opted_in() -> Self {
        let mut s = Self::new();
        s.opt_in_with_session().expect("opt in");
        s
    }

    /// The owner's policy signing a v9 frame.
    pub fn as_owner(&self, kind: TxKind, outputs: Vec<TxOutput>) -> Transaction {
        let key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes(
            [OWNER_SEED; 32],
        ));
        v9_frame(
            self.address,
            &self.owner_policy,
            kind,
            outputs,
            self.chain.nonce(&self.address),
            |m| {
                vec![Approval {
                    index: 0,
                    signature: HybridMlDsa65SlhDsa128s::sign(&key, m).unwrap(),
                }]
            },
        )
    }

    /// The session key signing a v9 frame.
    pub fn as_session(&self, kind: TxKind, outputs: Vec<TxOutput>) -> Transaction {
        v9_frame(
            self.address,
            &self.session_policy,
            kind,
            outputs,
            self.chain.nonce(&self.address),
            |m| {
                vec![Approval {
                    index: 0,
                    signature: MlDsa65::sign(&self.session, m).unwrap(),
                }]
            },
        )
    }

    pub fn session_pays(&mut self, recipient: Address, amount: u64) -> Result<(), String> {
        let tx = self.as_session(TxKind::Transfer, pay_to(recipient, amount));
        self.chain.send(tx)
    }

    pub fn owner_does(&mut self, action: AccountAction) -> Result<(), String> {
        let tx = self.as_owner(account(action), vec![]);
        self.chain.send(tx)
    }

    /// Switches fees on: a base fee of 1 per byte, floored at 1, so it stays
    /// 1 for these small blocks. `Chain::new` writes no fee record, so every
    /// test without this runs fee-free.
    pub fn fees_on(&self) {
        let record = FeeRecord {
            base_fee: Amount::from_u64(1),
            min_base_fee: Amount::from_u64(1),
            target_block_bytes: 1_000_000,
            change_denominator: 8,
        };
        self.chain
            .db
            .raw_put_for_test(FEE_KEY, &record.encode())
            .unwrap();
    }

    /// An owner-signed transfer to `STRANGER`, with a fee output.
    pub fn owner_pays(&self, amount: u64, fee: u64) -> Transaction {
        self.as_owner(TxKind::Transfer, with_fee(pay_to(STRANGER, amount), fee))
    }

    /// A session-signed transfer to `PAYEE`, with a fee output.
    pub fn session_pays_with_fee(&self, amount: u64, fee: u64) -> Transaction {
        self.as_session(TxKind::Transfer, with_fee(pay_to(PAYEE, amount), fee))
    }
}

/// Runs `f` and asserts it was refused with `needle` and moved nothing.
pub fn refused(s: &mut Scene, needle: &str, f: impl FnOnce(&mut Scene) -> Result<(), String>) {
    let nonce = s.chain.nonce(&s.address);
    let balance = s.chain.balance(&s.address);
    let err = f(s).unwrap_err();
    assert!(err.contains(needle), "expected `{needle}` in: {err}");
    assert_eq!(
        s.chain.nonce(&s.address),
        nonce,
        "a refused frame consumes no nonce"
    );
    assert_eq!(s.chain.balance(&s.address), balance);
}
