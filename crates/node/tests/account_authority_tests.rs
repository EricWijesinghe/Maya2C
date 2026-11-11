//! Account authority and guardian recovery (ADR-086 §3, §9), executed by the
//! node's own state transition. Dark: every test that exercises it sets the
//! activation height explicitly; the compiled height is `u64::MAX`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::account_payload::{AccountAction, GuardianSet};
use custom_l1_node::core::account_tx::signer_policy;
use custom_l1_node::core::block::{block_wire_len, first_fit, transaction_wire_len};
use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, ChainTag, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::suites::{SuiteSunset, SuiteSunsets, suite_address};
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::account_authority::MIN_DELAY_BLOCKS;
use custom_l1_node::state::{Account, Address, BlockContext, Module, StateDB};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_fee_market::limits::MAX_BLOCK_BYTES;
use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa65, MlDsa87, SignatureSuite, SuiteId,
};
use primitive_types::U256;
use std::sync::Arc;
use tempfile::TempDir;

const CHAIN: ChainTag = ChainTag::from_genesis([42; 32]);
const FUNDS: u64 = 10_000;
const ACTIVE: u64 = 1;
const DELAY: u64 = MIN_DELAY_BLOCKS;
const PAYEE: Address = [0xB0; 32];

type Key65 = <MlDsa65 as SignatureSuite>::SigningKey;
type Key87 = <MlDsa87 as SignatureSuite>::SigningKey;

// ---------------------------------------------------------------- fixtures --

fn device(seed: u8) -> Key65 {
    MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]))
}

fn devices(seeds: &[u8]) -> Vec<Key65> {
    seeds.iter().map(|&s| device(s)).collect()
}

fn policy_of(threshold: usize, keys: &[Key65]) -> MultisigPolicy {
    let listed = keys
        .iter()
        .map(|k| PolicyKey {
            suite: SuiteId::MlDsa65,
            public_key: MlDsa65::public_key(k),
        })
        .collect();
    MultisigPolicy::new(threshold, listed).expect("policy")
}

/// A guardian: an ML-DSA-87 (v7) key, the genesis default suite, which no
/// sunset can retire.
struct Guardian {
    key: Key87,
    address: Address,
}

fn guardian(seed: u8) -> Guardian {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]));
    let address = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&key));
    Guardian { key, address }
}

fn owner_key(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).expect("hybrid key")
}

struct Chain {
    db: StateDB,
    height: u64,
    activation: u64,
    sunsets: SuiteSunsets,
    _dir: TempDir,
}

impl Chain {
    fn new(funded: &[Address]) -> Self {
        Self::with_activation(funded, ACTIVE)
    }

    fn with_activation(funded: &[Address], activation: u64) -> Self {
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        db.bind_chain(CHAIN).unwrap();
        for a in funded {
            db.put_account(
                a,
                &Account {
                    balance: U256::from(FUNDS),
                    nonce: 0,
                },
            )
            .unwrap();
        }
        Self {
            db,
            height: 0,
            activation,
            sunsets: SuiteSunsets::EMPTY,
            _dir: dir,
        }
    }

    fn context(&self, height: u64) -> BlockContext {
        BlockContext::at_height(height)
            .with_account_authority_activation(self.activation)
            .with_suite_sunsets(self.sunsets)
    }

    fn block(&self, txs: Vec<Transaction>) -> Block {
        Block::new(
            BlockHeader {
                version: 0,
                prev_hash: [0; 32],
                state_root: [0; 32],
                timestamp: 1_790_000_000 + self.height,
                nonce: 0,
                difficulty_target: target_from_leading_zero_bits(0),
                tx_root: [0; 32],
            },
            txs,
        )
    }

    /// One transaction in the next block; `Err` if the block is refused.
    fn send(&mut self, tx: Transaction) -> Result<(), String> {
        self.height += 1;
        let block = self.block(vec![tx]);
        self.db
            .apply_block(&block, self.context(self.height))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn idle_to(&mut self, height: u64) {
        self.height = self.height.max(height);
    }

    fn nonce(&self, a: &Address) -> u64 {
        self.db.get_account(a).unwrap().nonce
    }

    fn balance(&self, a: &Address) -> U256 {
        self.db.get_account(a).unwrap().balance
    }

    fn authority(&self, a: &Address) -> Option<[u8; 32]> {
        let (record, _, _) = self.db.committed_account_authority(a).unwrap();
        record.map(|r| r.effective(self.height + 1))
    }
}

fn hybrid(chain: &Chain, key: &HybridSigningKey, kind: TxKind, outputs: Vec<TxOutput>) -> Transaction {
    let mut tx = Transaction::with_kind(kind, chain.nonce(&key.address()));
    tx.outputs = outputs;
    tx.sign(key, &CHAIN).unwrap();
    tx
}

fn as_guardian(chain: &Chain, g: &Guardian, action: AccountAction) -> Transaction {
    let mut tx = Transaction::with_kind(TxKind::Account(Box::new(action)), chain.nonce(&g.address));
    tx.sign_with_suite::<MlDsa87>(&g.key, &CHAIN).unwrap();
    tx
}

/// A wire-version-9 frame for `account`, approved by `signers` of `policy`.
#[allow(clippy::too_many_arguments)]
fn v9_with(
    account: Address,
    policy: &MultisigPolicy,
    keys: &[Key65],
    signers: &[u8],
    kind: TxKind,
    outputs: Vec<TxOutput>,
    nonce: u64,
    chain: &ChainTag,
) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.outputs = outputs;
    tx.multisig = Some(Box::new(
        MultisigAuth::unsigned(policy.clone()).bound_to(account),
    ));
    let message = tx.signing_bytes(chain);
    let approvals = signers
        .iter()
        .map(|&i| Approval {
            index: i,
            signature: MlDsa65::sign(&keys[usize::from(i)], &message).unwrap(),
        })
        .collect();
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy.clone(), approvals)
            .unwrap()
            .bound_to(account),
    ));
    tx
}

fn v9(
    chain: &Chain,
    account: Address,
    policy: &MultisigPolicy,
    keys: &[Key65],
    kind: TxKind,
    outputs: Vec<TxOutput>,
) -> Transaction {
    let signers: Vec<u8> = (0..u8::try_from(policy.threshold()).unwrap()).collect();
    v9_with(
        account,
        policy,
        keys,
        &signers,
        kind,
        outputs,
        chain.nonce(&account),
        &CHAIN,
    )
}

fn pay(amount: u64) -> Vec<TxOutput> {
    vec![TxOutput {
        amount: U256::from(amount),
        recipient: PAYEE,
    }]
}

fn account(action: AccountAction) -> TxKind {
    TxKind::Account(Box::new(action))
}

fn guardians(gs: &[&Guardian], threshold: u8) -> GuardianSet {
    GuardianSet {
        guardians: gs.iter().map(|g| g.address).collect(),
        threshold,
        delay_blocks: DELAY,
    }
}

/// A hybrid-key owner opted in with three guardians (2 of 3).
struct Scene {
    chain: Chain,
    owner: HybridSigningKey,
    address: Address,
    gs: [Guardian; 3],
    old: MultisigPolicy,
}

const OWNER_SEED: u8 = 0x01;

fn scene() -> Scene {
    let owner = owner_key(OWNER_SEED);
    let address = owner.address();
    let gs = [guardian(0x21), guardian(0x22), guardian(0x23)];
    let mut chain = Chain::new(&[address]);
    let opt_in = hybrid(
        &chain,
        &owner,
        account(AccountAction::SetGuardians(guardians(
            &[&gs[0], &gs[1], &gs[2]],
            2,
        ))),
        vec![],
    );
    let old = signer_policy(&opt_in).unwrap();
    chain.send(opt_in).expect("opt in");
    Scene {
        chain,
        owner,
        address,
        gs,
        old,
    }
}

fn propose(s: &mut Scene, g: usize, policy: &MultisigPolicy) -> Result<(), String> {
    let tx = as_guardian(
        &s.chain,
        &s.gs[g],
        AccountAction::ProposeRecovery {
            account: s.address,
            policy: policy.clone(),
        },
    );
    s.chain.send(tx)
}

fn finish(s: &mut Scene) -> Result<(), String> {
    // Anyone may finish; a guardian happens to be at hand.
    let tx = as_guardian(
        &s.chain,
        &s.gs[2],
        AccountAction::FinishRecovery { account: s.address },
    );
    s.chain.send(tx)
}

// ------------------------------------------------------------------- tests --

#[test]
fn an_opt_in_records_the_signing_policy_and_moves_the_authority() {
    let s = scene();
    assert_eq!(s.chain.authority(&s.address), Some(s.old.digest()));
    // Rule A: the owner's own hybrid key no longer acts as a v6 frame...
    let mut s = s;
    let v6 = hybrid(&s.chain, &s.owner, TxKind::Transfer, pay(5));
    let err = s.chain.send(v6).unwrap_err();
    assert!(err.contains("authority moved"), "{err}");
}

#[test]
fn nothing_changes_before_quorum_and_delay() {
    let mut s = scene();
    let new = policy_of(2, &devices(&[1, 2, 3]));
    propose(&mut s, 0, &new).unwrap();
    assert!(finish(&mut s).unwrap_err().contains("no quorum"));
    propose(&mut s, 1, &new).unwrap();
    let quorum_at = s.chain.height;
    s.chain.idle_to(quorum_at + DELAY - 2);
    let err = finish(&mut s).unwrap_err();
    assert!(err.contains("matures"), "{err}");
    assert_eq!(s.chain.authority(&s.address), Some(s.old.digest()));
    s.chain.idle_to(quorum_at + DELAY - 1);
    finish(&mut s).expect("matured");
    assert_eq!(s.chain.authority(&s.address), Some(new.digest()));
}

#[test]
fn a_cancel_ends_the_request() {
    let mut s = scene();
    let new = policy_of(2, &devices(&[1, 2, 3]));
    propose(&mut s, 0, &new).unwrap();
    propose(&mut s, 1, &new).unwrap();
    // The owner, under its recorded 1-of-1 policy, cancels as a v9 frame.
    let cancel = v9_hybrid(
        &s.chain,
        OWNER_SEED,
        &s.old,
        s.address,
        account(AccountAction::CancelRecovery { account: s.address }),
    );
    s.chain.send(cancel).expect("cancel");
    let (_, _, recovery) = s.chain.db.committed_account_authority(&s.address).unwrap();
    assert_eq!(recovery, None);
    s.chain.idle_to(s.chain.height + DELAY + 10);
    assert!(finish(&mut s).unwrap_err().contains("no recovery"));
    assert_eq!(s.chain.authority(&s.address), Some(s.old.digest()));
}

/// The owner's hybrid key as a 1-of-1 v9 frame.
fn v9_hybrid(
    chain: &Chain,
    seed: u8,
    policy: &MultisigPolicy,
    account: Address,
    kind: TxKind,
) -> Transaction {
    let mut tx = Transaction::with_kind(kind, chain.nonce(&account));
    tx.multisig = Some(Box::new(
        MultisigAuth::unsigned(policy.clone()).bound_to(account),
    ));
    let message = tx.signing_bytes(&CHAIN);
    // The node's hybrid key from this seed is the suite-0x30 key from it
    // (invariant 30), so the owner signs its own 1-of-1 policy.
    let suite_key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]));
    let approvals = vec![Approval {
        index: 0,
        signature: HybridMlDsa65SlhDsa128s::sign(&suite_key, &message).unwrap(),
    }];
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy.clone(), approvals)
            .unwrap()
            .bound_to(account),
    ));
    tx
}

#[test]
fn old_key_frames_are_refused_after_recovery() {
    let mut s = scene();
    // Pre-signed by the thief before recovery, for the nonce it will need.
    let mut presigned = Transaction::new(vec![], pay(100), s.chain.nonce(&s.address));
    presigned.sign(&s.owner, &CHAIN).unwrap();
    let stale_v9 = v9_hybrid(&s.chain, OWNER_SEED, &s.old, s.address, TxKind::Transfer);

    let keys = devices(&[1, 2, 3]);
    let new = policy_of(2, &keys);
    propose(&mut s, 0, &new).unwrap();
    propose(&mut s, 1, &new).unwrap();
    s.chain.idle_to(s.chain.height + DELAY);
    finish(&mut s).expect("recovered");

    let err = s.chain.send(presigned).unwrap_err();
    assert!(err.contains("authority moved"), "v6: {err}");
    let err = s.chain.send(stale_v9).unwrap_err();
    assert!(err.contains("not the one signing"), "stale v9: {err}");
    let fresh = v9(&s.chain, s.address, &new, &keys, TxKind::Transfer, pay(7));
    s.chain.send(fresh).expect("the new keys spend");
    assert_eq!(s.chain.balance(&PAYEE), U256::from(7));
}

#[test]
fn v7_and_v8_frames_are_refused_once_their_account_opts_in() {
    let gs = [guardian(0x31), guardian(0x32), guardian(0x33)];
    let set = guardians(&[&gs[0], &gs[1], &gs[2]], 2);

    // v7: an ML-DSA-65 single-key account.
    let k7 = device(0x07);
    let a7 = suite_address(SuiteId::MlDsa65, &MlDsa65::public_key(&k7));
    // v8: a 2-of-3 multisig account.
    let k8 = devices(&[0x81, 0x82, 0x83]);
    let p8 = policy_of(2, &k8);
    let a8 = multisig_address(&p8);
    let mut chain = Chain::new(&[a7, a8]);

    let mut tx = Transaction::with_kind(account(AccountAction::SetGuardians(set.clone())), 0);
    tx.sign_with_suite::<MlDsa65>(&k7, &CHAIN).unwrap();
    chain.send(tx).expect("v7 opt in");
    let mut v7 = Transaction::new(vec![], pay(1), chain.nonce(&a7));
    v7.sign_with_suite::<MlDsa65>(&k7, &CHAIN).unwrap();
    assert!(chain.send(v7).unwrap_err().contains("authority moved"));
    let p7 = MultisigPolicy::new(
        1,
        vec![PolicyKey {
            suite: SuiteId::MlDsa65,
            public_key: MlDsa65::public_key(&k7),
        }],
    )
    .unwrap();
    let ok = v9(&chain, a7, &p7, std::slice::from_ref(&k7), TxKind::Transfer, pay(1));
    chain.send(ok).expect("the v7 key acts through its 1-of-1 v9 frame");

    let mut tx = Transaction::with_kind(account(AccountAction::SetGuardians(set)), 0);
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(p8.clone())));
    let message = tx.signing_bytes(&CHAIN);
    let approvals = (0..2u8)
        .map(|i| Approval {
            index: i,
            signature: MlDsa65::sign(&k8[usize::from(i)], &message).unwrap(),
        })
        .collect();
    tx.multisig = Some(Box::new(MultisigAuth::new(p8.clone(), approvals).unwrap()));
    let mut v8_later = tx.clone();
    chain.send(tx).expect("v8 opt in");
    assert_eq!(chain.authority(&a8), Some(p8.digest()));
    v8_later.nonce = chain.nonce(&a8); // re-signed below at the right nonce
    v8_later.multisig = Some(Box::new(MultisigAuth::unsigned(p8.clone())));
    v8_later.kind = TxKind::Transfer;
    v8_later.outputs = pay(1);
    let message = v8_later.signing_bytes(&CHAIN);
    let approvals = (0..2u8)
        .map(|i| Approval {
            index: i,
            signature: MlDsa65::sign(&k8[usize::from(i)], &message).unwrap(),
        })
        .collect();
    v8_later.multisig = Some(Box::new(MultisigAuth::new(p8.clone(), approvals).unwrap()));
    assert!(chain.send(v8_later).unwrap_err().contains("authority moved"));
    chain
        .send(v9(&chain, a8, &p8, &k8, TxKind::Transfer, pay(1)))
        .expect("the same quorum acts through v9");
}

#[test]
fn account_actions_write_no_balance() {
    let mut s = scene();
    let before: Vec<U256> = std::iter::once(s.address)
        .chain(s.gs.iter().map(|g| g.address))
        .map(|a| s.chain.balance(&a))
        .collect();
    let owner_nonce = s.chain.nonce(&s.address);
    let new = policy_of(2, &devices(&[1, 2, 3]));
    propose(&mut s, 0, &new).unwrap();
    propose(&mut s, 1, &new).unwrap();
    s.chain.idle_to(s.chain.height + DELAY);
    finish(&mut s).unwrap();
    let after: Vec<U256> = std::iter::once(s.address)
        .chain(s.gs.iter().map(|g| g.address))
        .map(|a| s.chain.balance(&a))
        .collect();
    assert_eq!(before, after, "no account action moves value");
    assert_eq!(
        s.chain.nonce(&s.address),
        owner_nonce,
        "finishing does not touch the recovered account's nonce"
    );
    // An account action carrying a value output is refused outright.
    let gs = &s.gs[0];
    let mut tx = Transaction::with_kind(
        account(AccountAction::WithdrawApproval { account: s.address }),
        s.chain.nonce(&gs.address),
    );
    tx.outputs = pay(1);
    tx.sign_with_suite::<MlDsa87>(&gs.key, &CHAIN).unwrap();
    assert!(s.chain.send(tx).is_err());
}

#[test]
fn a_later_guardian_change_is_queued_behind_the_current_delay() {
    let mut s = scene();
    let fresh = [guardian(0x41), guardian(0x42), guardian(0x43)];
    let change = v9_hybrid(
        &s.chain,
        OWNER_SEED,
        &s.old,
        s.address,
        account(AccountAction::SetGuardians(guardians(
            &[&fresh[0], &fresh[1], &fresh[2]],
            2,
        ))),
    );
    s.chain.send(change).expect("queued");
    let queued_at = s.chain.height;
    let (_, g, _) = s.chain.db.committed_account_authority(&s.address).unwrap();
    let g = g.unwrap();
    assert_eq!(g.effective(queued_at + DELAY - 1).guardians[0], s.gs[0].address);
    assert_eq!(g.effective(queued_at + DELAY).guardians[0], fresh[0].address);
    // While queued the old guardians still recover.
    let new = policy_of(2, &devices(&[1, 2, 3]));
    propose(&mut s, 0, &new).expect("old guardian still counts");
}

#[test]
fn one_guardian_cannot_block_a_majority() {
    let mut s = scene();
    let junk = policy_of(1, &devices(&[0x66]));
    let good = policy_of(2, &devices(&[1, 2, 3]));
    propose(&mut s, 0, &junk).expect("the hostile guardian proposes first");
    propose(&mut s, 1, &good).unwrap();
    propose(&mut s, 2, &good).unwrap();
    s.chain.idle_to(s.chain.height + DELAY);
    finish(&mut s).expect("the majority wins");
    assert_eq!(s.chain.authority(&s.address), Some(good.digest()));
}

#[test]
fn a_moved_or_withdrawn_approval_stops_the_clock() {
    let mut s = scene();
    let good = policy_of(2, &devices(&[1, 2, 3]));
    let other = policy_of(2, &devices(&[4, 5, 6]));
    propose(&mut s, 0, &good).unwrap();
    propose(&mut s, 1, &good).unwrap();
    // Guardian 1 changes its mind: quorum is gone.
    propose(&mut s, 1, &other).unwrap();
    s.chain.idle_to(s.chain.height + DELAY);
    assert!(finish(&mut s).unwrap_err().contains("no quorum"));
    let tx = as_guardian(
        &s.chain,
        &s.gs[1],
        AccountAction::WithdrawApproval { account: s.address },
    );
    s.chain.send(tx).unwrap();
    let (_, _, r) = s.chain.db.committed_account_authority(&s.address).unwrap();
    let r = r.unwrap();
    assert_eq!(r.approvals.len(), 1);
    assert_eq!(r.policies.len(), 1, "an unapproved policy is dropped");
}

#[test]
fn removed_guardians_approvals_are_ignored() {
    let mut s = scene();
    let new = policy_of(2, &devices(&[1, 2, 3]));
    propose(&mut s, 0, &new).unwrap();
    // Owner replaces guardians 0 and 1; the change matures.
    let fresh = [guardian(0x51), guardian(0x52)];
    let change = v9_hybrid(
        &s.chain,
        OWNER_SEED,
        &s.old,
        s.address,
        account(AccountAction::SetGuardians(guardians(
            &[&fresh[0], &fresh[1], &s.gs[2]],
            2,
        ))),
    );
    s.chain.send(change).unwrap();
    s.chain.idle_to(s.chain.height + DELAY);
    // Guardian 0 is no longer a guardian at all.
    assert!(propose(&mut s, 0, &new).unwrap_err().contains("current guardian"));
    // Guardian 2 approves: with 0's approval it would be 2 of 3, without it 1.
    propose(&mut s, 2, &new).unwrap();
    s.chain.idle_to(s.chain.height + DELAY);
    assert!(finish(&mut s).unwrap_err().contains("no quorum"));
}

#[test]
fn guardian_bounds_are_enforced_by_the_chain() {
    let owner = owner_key(0x02);
    let address = owner.address();
    let gs = [guardian(0x61), guardian(0x62), guardian(0x63)];
    let mut chain = Chain::new(&[address]);
    let all = [&gs[0], &gs[1], &gs[2]];
    let bad = [
        GuardianSet {
            threshold: 0,
            ..guardians(&all, 2)
        },
        GuardianSet {
            threshold: 4,
            ..guardians(&all, 2)
        },
        GuardianSet {
            threshold: 1,
            ..guardians(&all, 2)
        },
        GuardianSet {
            delay_blocks: 0,
            ..guardians(&all, 2)
        },
        GuardianSet {
            guardians: vec![gs[0].address, gs[0].address, gs[1].address],
            ..guardians(&all, 2)
        },
        GuardianSet {
            guardians: vec![address, gs[0].address, gs[1].address],
            ..guardians(&all, 2)
        },
    ];
    for set in bad {
        let tx = hybrid(&chain, &owner, account(AccountAction::SetGuardians(set.clone())), vec![]);
        assert!(chain.send(tx).is_err(), "{set:?} must be refused");
    }
    assert_eq!(chain.authority(&address), None, "a refused opt-in records nothing");
}

#[test]
fn cancel_and_withdraw_are_never_halted() {
    let a = [7; 32];
    assert_eq!(Module::of(&account(AccountAction::CancelRecovery { account: a })), None);
    assert_eq!(Module::of(&account(AccountAction::WithdrawApproval { account: a })), None);
    assert_eq!(
        Module::of(&account(AccountAction::FinishRecovery { account: a })),
        Some(Module::Account)
    );
    assert_eq!(
        Module::of(&account(AccountAction::ProposeRecovery {
            account: a,
            policy: policy_of(1, &devices(&[9])),
        })),
        Some(Module::Account)
    );
}

#[test]
fn v9_and_account_actions_are_refused_below_activation_and_by_verify() {
    let keys = devices(&[1, 2, 3]);
    let policy = policy_of(2, &keys);
    let account_address = [0xAC; 32];
    let owner = owner_key(0x03);
    let mut chain = Chain::with_activation(&[account_address, owner.address()], 1_000);

    let frame = v9(&chain, account_address, &policy, &keys, TxKind::Transfer, pay(1));
    assert!(frame.verify(&CHAIN).is_err(), "verify has no height");
    let err = chain.send(frame.clone()).unwrap_err();
    assert!(err.contains("not active"), "{err}");
    let gs = [guardian(0x71), guardian(0x72), guardian(0x73)];
    let opt_in = hybrid(
        &chain,
        &owner,
        account(AccountAction::SetGuardians(guardians(&[&gs[0], &gs[1], &gs[2]], 2))),
        vec![],
    );
    let err = chain.send(opt_in).unwrap_err();
    assert!(err.contains("not active"), "{err}");

    // At activation, a v9 frame for an account nobody bound is still refused.
    chain.idle_to(999);
    let err = chain.send(frame).unwrap_err();
    assert!(err.contains("no authority record"), "{err}");

    // The mempool judges at the tip under the committed (dark) schedule.
    let pool = Mempool::new(Arc::new(chain.db));
    let frame = v9_with(account_address, &policy, &keys, &[0, 1], TxKind::Transfer, pay(1), 0, &CHAIN);
    let err = pool.validate(&frame).unwrap_err().to_string();
    assert!(err.contains("not active"), "{err}");
}

#[test]
fn v9_signing_bytes_bind_account_policy_and_chain() {
    let keys = devices(&[1, 2, 3]);
    let policy = policy_of(2, &keys);
    let other = policy_of(2, &devices(&[4, 5, 6]));
    let frame = v9_with([1; 32], &policy, &keys, &[0, 1], TxKind::Transfer, pay(3), 0, &CHAIN);
    let bytes = frame.signing_bytes(&CHAIN);
    let rebind = |account| {
        let mut t = frame.clone();
        let auth = t.multisig.take().unwrap();
        t.multisig = Some(Box::new(
            MultisigAuth::new(auth.policy().clone(), auth.approvals().to_vec())
                .unwrap()
                .bound_to(account),
        ));
        t
    };
    assert_ne!(rebind([2; 32]).signing_bytes(&CHAIN), bytes, "account");
    assert_ne!(frame.signing_bytes(&ChainTag::from_genesis([43; 32])), bytes, "chain");
    let mut swapped = frame.clone();
    swapped.multisig = Some(Box::new(MultisigAuth::unsigned(other).bound_to([1; 32])));
    assert_ne!(swapped.signing_bytes(&CHAIN), bytes, "policy");

    let policy_suites = custom_l1_node::crypto::suites::verification_policy(&SuiteSunsets::EMPTY)
        .unwrap();
    frame.verify_at(1, &policy_suites, &CHAIN).expect("as signed");
    assert!(rebind([2; 32]).verify_at(1, &policy_suites, &CHAIN).is_err());
    assert!(frame.verify_at(1, &policy_suites, &ChainTag::from_genesis([43; 32])).is_err());
    // The same approvals presented as a v8 frame do not verify: other domain.
    let mut as_v8 = frame.clone();
    let auth = as_v8.multisig.take().unwrap();
    as_v8.multisig = Some(Box::new(
        MultisigAuth::new(auth.policy().clone(), auth.approvals().to_vec()).unwrap(),
    ));
    assert!(as_v8.verify_at(1, &policy_suites, &CHAIN).is_err());

    // The wire round-trips, version 9, and the txid differs from the v8 form.
    let wire = frame.to_bytes();
    assert_eq!(wire[0], 9);
    assert_eq!(Transaction::from_bytes(&wire).unwrap(), frame);
    assert_ne!(frame.txid(), as_v8.txid());
    assert_eq!(frame.sender(), [1; 32]);
}

#[test]
fn a_suite_sunset_before_finish_refuses_the_recovery() {
    let mut s = scene();
    // ML-DSA-65 is announced for retirement at 100 and stops signing at
    // 100 + the emergency window.
    let sunset_height = 100 + maya_crypto_pq::agility::MIN_EMERGENCY_WINDOW;
    s.chain.sunsets = SuiteSunsets::new(&[SuiteSunset {
        suite: SuiteId::MlDsa65,
        announced_at: 100,
        sunset_height,
        emergency: true,
    }])
    .unwrap();
    let new = policy_of(2, &devices(&[1, 2, 3]));
    // A proposal whose delay would end past the sunset is refused...
    s.chain.idle_to(sunset_height - DELAY);
    assert!(propose(&mut s, 0, &new).unwrap_err().contains("may not sign"));
    // ...one that ends before it is admitted, and finishing late is refused.
    let mut s2 = scene();
    s2.chain.sunsets = s.chain.sunsets;
    s2.chain.idle_to(sunset_height - DELAY - 10);
    propose(&mut s2, 0, &new).unwrap();
    propose(&mut s2, 1, &new).unwrap();
    s2.chain.idle_to(sunset_height);
    let err = finish(&mut s2).unwrap_err();
    assert!(err.contains("may not sign"), "{err}");
}

fn transfer_roots(activation: u64) -> Vec<[u8; 32]> {
    let owner = owner_key(0x04);
    let mut chain = Chain::with_activation(&[owner.address()], activation);
    let mut roots = vec![chain.db.state_root().unwrap()];
    for amount in [3, 4] {
        let tx = hybrid(&chain, &owner, TxKind::Transfer, pay(amount));
        chain.send(tx).unwrap();
        roots.push(chain.db.state_root().unwrap());
    }
    roots
}

#[test]
fn the_root_is_unchanged_when_no_account_opts_in() {
    assert_eq!(transfer_roots(ACTIVE), transfer_roots(u64::MAX));
}

#[test]
fn a_reorg_restores_authority_records() {
    let owner = owner_key(0x05);
    let address = owner.address();
    let gs = [guardian(0x81), guardian(0x82), guardian(0x83)];
    let mut chain = Chain::new(&[address]);
    let before = chain.db.state_root().unwrap();
    let tx = hybrid(
        &chain,
        &owner,
        account(AccountAction::SetGuardians(guardians(&[&gs[0], &gs[1], &gs[2]], 2))),
        vec![],
    );
    chain.height = 1;
    let mut block = chain.block(vec![tx]);
    let context = chain.context(1);
    block.header.state_root = chain.db.preview_root(&block, context).unwrap();
    let after = chain.db.apply_block_journaled(&block, &[1; 32], context).unwrap();
    assert_ne!(after, before, "an opt-in moves the root");
    assert!(chain.authority(&address).is_some());
    assert_eq!(chain.db.uncovered_keys().unwrap(), Vec::<Vec<u8>>::new());

    chain.db.revert_block(&[1; 32]).unwrap();
    assert_eq!(chain.db.committed_account_authority(&address).unwrap(), (None, None, None));
    assert_eq!(chain.db.state_root().unwrap(), before);
}

#[test]
fn vault_limits_survive_recovery() {
    const LIMIT: u64 = 100;
    let owner = owner_key(0x06);
    let address = owner.address();
    let gs = [guardian(0x91), guardian(0x92), guardian(0x93)];
    let mut chain = Chain::new(&[address]);
    let vault = hybrid(
        &chain,
        &owner,
        TxKind::Vault(Box::new(VaultAction::Configure(VaultConfig {
            delay_blocks: 10,
            limit: U256::from(LIMIT),
            guardians: vec![gs[0].address],
        }))),
        vec![],
    );
    chain.send(vault).unwrap();
    let opt_in = hybrid(
        &chain,
        &owner,
        account(AccountAction::SetGuardians(guardians(&[&gs[0], &gs[1], &gs[2]], 2))),
        vec![],
    );
    chain.send(opt_in).expect("a vault account may opt in");
    let vault_before = chain.db.committed_vault(&address).unwrap();

    let keys = devices(&[1, 2, 3]);
    let new = policy_of(2, &keys);
    let mut s = Scene {
        chain,
        old: policy_of(1, &devices(&[0xEE])),
        owner,
        address,
        gs,
    };
    propose(&mut s, 0, &new).unwrap();
    propose(&mut s, 1, &new).unwrap();
    s.chain.idle_to(s.chain.height + DELAY);
    finish(&mut s).unwrap();
    assert_eq!(s.chain.db.committed_vault(&address).unwrap(), vault_before);

    let over = v9(&s.chain, address, &new, &keys, TxKind::Transfer, pay(LIMIT + 1));
    let err = s.chain.send(over).unwrap_err();
    assert!(err.contains("vault"), "{err}");
    let within = v9(&s.chain, address, &new, &keys, TxKind::Transfer, pay(LIMIT));
    s.chain.send(within).expect("in-limit transfer under the new keys");
}

#[test]
fn measured_v9_frame_bytes() {
    // Printed, not asserted: there is no target (ADR-086 §3.2, invariant 33).
    for (label, m, n) in [("2-of-3", 2usize, 3u8), ("3-of-5", 3, 5)] {
        let keys: Vec<_> = (1..=n)
            .map(|i| HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([i; 32])))
            .collect();
        let listed = keys
            .iter()
            .map(|k| PolicyKey {
                suite: SuiteId::HybridMlDsa65SlhDsa128s,
                public_key: HybridMlDsa65SlhDsa128s::public_key(k),
            })
            .collect();
        let policy = MultisigPolicy::new(m, listed).unwrap();
        let mut tx = Transaction::new(vec![], pay(1), 0);
        tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone()).bound_to([1; 32])));
        let message = tx.signing_bytes(&CHAIN);
        let approvals = (0..u8::try_from(m).unwrap())
            .map(|i| Approval {
                index: i,
                signature: HybridMlDsa65SlhDsa128s::sign(&keys[usize::from(i)], &message).unwrap(),
            })
            .collect();
        tx.multisig = Some(Box::new(
            MultisigAuth::new(policy, approvals).unwrap().bound_to([1; 32]),
        ));
        let v9_len = tx.to_bytes().len();
        let mut v6 = Transaction::new(vec![], pay(1), 0);
        v6.sign(&owner_key(0x0F), &CHAIN).unwrap();
        println!(
            "v9 frame, suite 0x30 {label}, one output: {v9_len} bytes (v6 single-key transfer: {} bytes)",
            v6.to_bytes().len()
        );
    }
}

// ------------------------------------------------------ block byte cap --

type HybridKey = <HybridMlDsa65SlhDsa128s as SignatureSuite>::SigningKey;

fn hybrid_devices(n: u8) -> Vec<HybridKey> {
    (1..=n)
        .map(|i| HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([i; 32])))
        .collect()
}

fn hybrid_policy(threshold: usize, keys: &[HybridKey]) -> MultisigPolicy {
    let listed = keys
        .iter()
        .map(|k| PolicyKey {
            suite: SuiteId::HybridMlDsa65SlhDsa128s,
            public_key: HybridMlDsa65SlhDsa128s::public_key(k),
        })
        .collect();
    MultisigPolicy::new(threshold, listed).expect("policy")
}

/// A signed v9 transfer of `account` at `nonce`, approved by the first `threshold` devices.
fn hybrid_frame(
    account: Address,
    policy: &MultisigPolicy,
    keys: &[HybridKey],
    nonce: u64,
) -> Transaction {
    let mut tx = Transaction::with_kind(TxKind::Transfer, nonce);
    tx.outputs = pay(1);
    tx.multisig = Some(Box::new(
        MultisigAuth::unsigned(policy.clone()).bound_to(account),
    ));
    let message = tx.signing_bytes(&CHAIN);
    let approvals = (0..u8::try_from(policy.threshold()).unwrap())
        .map(|i| Approval {
            index: i,
            signature: HybridMlDsa65SlhDsa128s::sign(&keys[usize::from(i)], &message).unwrap(),
        })
        .collect();
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy.clone(), approvals)
            .unwrap()
            .bound_to(account),
    ));
    tx
}

/// Frames from one recovered account fill a block to the cap. The count is
/// derived from the measured frame size, and the selection path is the one the
/// producer runs. The frame that does not fit is carried into the next block.
#[test]
fn v9_frames_fill_the_block_cap_and_the_rest_carry_over() {
    for (label, m, n) in [("2-of-3", 2usize, 3u8), ("3-of-5", 3, 5)] {
        let mut s = scene();
        let keys = hybrid_devices(n);
        let new = hybrid_policy(m, &keys);
        propose(&mut s, 0, &new).unwrap();
        propose(&mut s, 1, &new).unwrap();
        s.chain.idle_to(s.chain.height + DELAY);
        finish(&mut s).expect("recovered");

        let base = s.chain.nonce(&s.address);
        let probe = hybrid_frame(s.address, &new, &keys, base);
        let size = probe.to_bytes().len();
        let room = MAX_BLOCK_BYTES - block_wire_len([]);
        let fit = usize::try_from(room / transaction_wire_len(size)).unwrap();
        let frames: Vec<Transaction> = (0..=fit as u64)
            .map(|i| hybrid_frame(s.address, &new, &keys, base + i))
            .collect();
        let lens: Vec<usize> = frames.iter().map(|t| t.to_bytes().len()).collect();
        println!(
            "v9 {label}: measured {size} bytes per frame; first_fit room {room} bytes admits {fit}; \
             block of {fit} = {} bytes; block of {} = {} bytes (cap {MAX_BLOCK_BYTES})",
            block_wire_len(lens[..fit].iter().copied()),
            fit + 1,
            block_wire_len(lens.iter().copied()),
        );

        assert_eq!(
            first_fit(&lens, MAX_BLOCK_BYTES),
            (0..fit).collect::<Vec<_>>()
        );
        assert!(block_wire_len(lens[..fit].iter().copied()) <= MAX_BLOCK_BYTES);
        assert!(block_wire_len(lens.iter().copied()) > MAX_BLOCK_BYTES);

        let height = s.chain.height + 1;
        let kept = s
            .chain
            .db
            .select_applicable_within(
                frames.clone(),
                s.chain.context(height),
                target_from_leading_zero_bits(0),
                MAX_BLOCK_BYTES,
            )
            .unwrap();
        assert_eq!(kept.len(), fit, "{label}: selection admits the floor count");
        let first = s.chain.block(kept);
        assert_eq!(
            first.to_bytes().len() as u64,
            block_wire_len(lens[..fit].iter().copied())
        );
        s.chain.height = height;
        s.chain
            .db
            .apply_block(&first, s.chain.context(height))
            .expect("the full block imports");

        let carried = s.chain.block(vec![frames[fit].clone()]);
        s.chain.height += 1;
        s.chain
            .db
            .apply_block(&carried, s.chain.context(s.chain.height))
            .expect("the carried frame imports in the next block");
        assert_eq!(s.chain.nonce(&s.address), base + fit as u64 + 1);
        assert_eq!(s.chain.balance(&PAYEE), U256::from(fit as u64 + 1));
    }
}

// ----------------------------------------------------- conformance vectors --

const VECTORS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../spec/account-authority-vectors.json"
);

fn vector_cases() -> serde_json::Value {
    let keys = devices(&[1, 2, 3]);
    let policy = policy_of(2, &keys);
    let frame = v9_with([1; 32], &policy, &keys, &[0, 1], TxKind::Transfer, pay(3), 0, &CHAIN);
    let set = GuardianSet {
        guardians: vec![[0x11; 32], [0x22; 32], [0x33; 32]],
        threshold: 2,
        delay_blocks: DELAY,
    };
    let mut kind = Vec::new();
    account(AccountAction::SetGuardians(set)).encode_into(&mut kind);
    let mut cancel = Vec::new();
    account(AccountAction::CancelRecovery { account: [1; 32] }).encode_into(&mut cancel);
    let hex = |b: &[u8]| hex::encode(b);
    serde_json::json!({
        "description": "ADR-086 account authority: v9 signing bytes, payload encodings and refusals. Regenerate with MAYA_REGEN_VECTORS=1 cargo test -p custom-l1-node --test account_authority_tests conformance; every implementation must agree.",
        "cases": [
            {
                "id": "v9-signing-bytes",
                "rules": ["AA-2"],
                "input": {"chain_tag": hex(&CHAIN.0), "account": hex(&[1u8; 32]), "policy": hex(&policy.encode()), "nonce": 0, "outputs": [{"to": hex(&PAYEE), "amount": "3"}]},
                "expect": {"result": "ok", "policy_digest": hex(&policy.digest()), "signing_bytes_blake3": hex(blake3::hash(&frame.signing_bytes(&CHAIN)).as_bytes()), "txid": hex(&frame.txid())}
            },
            {
                "id": "account-payload-set-guardians",
                "rules": ["AA-3"],
                "expect": {"result": "ok", "kind_bytes": hex(&kind)}
            },
            {
                "id": "account-payload-cancel",
                "rules": ["AA-3"],
                "expect": {"result": "ok", "kind_bytes": hex(&cancel)}
            },
            {
                "id": "v9-below-activation",
                "rules": ["AA-4"],
                "expect": {"result": "error", "error": "Account"}
            },
            {
                "id": "v9-without-record",
                "rules": ["AA-1"],
                "expect": {"result": "error", "error": "AuthorityMoved"}
            },
            {
                "id": "v6-after-opt-in",
                "rules": ["AA-1"],
                "expect": {"result": "error", "error": "AuthorityMoved"}
            },
            {
                "id": "root-without-opt-in",
                "rules": ["AA-5"],
                "expect": {"result": "ok", "roots_equal_dark_chain": transfer_roots(ACTIVE) == transfer_roots(u64::MAX)}
            }
        ]
    })
}

#[test]
fn conformance_vectors_match_the_spec_file() {
    let computed = vector_cases();
    if std::env::var_os("MAYA_REGEN_VECTORS").is_some() {
        let text = serde_json::to_string_pretty(&computed).unwrap() + "\n";
        std::fs::write(VECTORS_PATH, text).unwrap();
    }
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(VECTORS_PATH).unwrap()).unwrap();
    assert_eq!(stored, computed, "spec/account-authority-vectors.json is stale");
}
