//! Golden state roots: the same blocks give the same roots on every OS.
//!
//! The genesis validator runs on Windows and the others may not, so a state
//! transition that computes differently on one platform (an endianness slip,
//! a `usize` width, a platform-dependent iteration order) would split the
//! chain at the first block that hits it. Nothing else here would notice: the
//! rest of the suite derives its expected roots on the machine it runs on.
//!
//! This test replays a committed chain — transfers, staking, DEX, a contract
//! and a governance proposal carried through to execution — through
//! `StateDB::apply_block_journaled`, the path `Chain` uses (invariant 24), and
//! compares every block id and state root with committed golden values. CI
//! runs it on Linux, Windows and macOS against the same two files, so the
//! three agree with each other only by agreeing with the file.
//!
//! The block bytes are committed and replayed as bytes, so the replay does not
//! depend on this file building the same transactions. A second test rebuilds
//! the chain in code and compares it with the committed bytes: key derivation
//! and both signature schemes are deterministic, so that pins them across
//! platforms too. Regenerate both files with `MAYA_BLESS_GOLDEN=1` after a
//! deliberate consensus change, and review the diff of `expected.txt`: every
//! root that moved is a state transition that changed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use custom_l1_node::core::dex_payload::{
    AssetRegistration, LiquidityDeposit, PoolCreation, SwapRequest,
};
use custom_l1_node::core::governance_payload::{Ballot, ProposalSubmission, StakeLock};
use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::core::{Block, BlockHeader, ChainTag, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys as keys;
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
use custom_l1_node::governance::derive_proposal_id;
use custom_l1_node::state::staking::validator_id;
use custom_l1_node::state::{
    BlockContext, Direction, NATIVE_ASSET, StateDB, derive_asset_id, derive_pair_id,
};
use maya_governance::limits::{MIN_TIMELOCK_BLOCKS, MIN_VOTING_BLOCKS};
use maya_governance::params::ParameterKey;
use maya_governance::proposal::ProposalState;
use maya_governance::tally::Choice;
use primitive_types::U256;
use tempfile::TempDir;

const BLESS: &str = "MAYA_BLESS_GOLDEN";
const GENESIS_VALIDATORS: u8 = 4;
const EPOCH_BLOCKS: u64 = 4;
const LP_FEE_BPS: u32 = 30;
const GOLD: [u8; 8] = *b"GLD\0\0\0\0\0";

/// One coin at 24 decimals: balances wide enough to exercise the U256 paths.
fn coins(n: u64) -> U256 {
    U256::from(n) * U256::exp10(24)
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden_chain")
}

fn alice() -> HybridSigningKey {
    signing_key_from_seed(&[0xA1; 32]).unwrap()
}
fn bob() -> HybridSigningKey {
    signing_key_from_seed(&[0xB2; 32]).unwrap()
}
fn carol() -> HybridSigningKey {
    signing_key_from_seed(&[0xC3; 32]).unwrap()
}

fn validator(i: u8) -> keys::SigningKey {
    keys::signing_key_from_seed(&[i + 1; 32]).unwrap()
}

/// A DAG-BFT genesis with stake-weighted staking: the shape mainnet launches
/// with, minus fees and mining, which would make every transaction carry a
/// fee output and is pinned by the fee-market and mining vectors instead.
fn genesis() -> GenesisConfig {
    let operator = hex::encode(alice().address());
    GenesisConfig {
        chain_id: "golden-root".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: [alice(), bob(), carol()]
            .iter()
            .map(|k| Allocation {
                address: hex::encode(k.address()),
                balance: coins(1_000_000),
            })
            .collect(),
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators: (0..GENESIS_VALIDATORS)
                .map(|i| hex::encode(validator(i).verifying_key().to_bytes()))
                .collect(),
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 0,
            fees: None,
            mining: None,
            staking: Some(StakingGenesis {
                epoch_blocks: EPOCH_BLOCKS,
                bonds: (0..GENESIS_VALIDATORS)
                    .map(|_| GenesisBond {
                        operator: operator.clone(),
                        bond: coins(10_000),
                        commission_bps: 500,
                    })
                    .collect(),
                min_self_bond: coins(1_000),
                max_validators: 100,
                min_delegation: coins(10),
                stake_weighted: true,
            }),
        }),
    }
}

/// A fresh state seeded with [`genesis`], bound to its chain.
fn seeded() -> (TempDir, StateDB, Block, [u8; 32]) {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    let config = genesis();
    let root = config.seed_state(&db).unwrap();
    let block = config.genesis_block().unwrap();
    db.bind_chain(ChainTag::from_genesis(block.header.id()))
        .unwrap();
    (dir, db, block, root)
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey, chain: &ChainTag) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, chain).unwrap();
    tx
}

fn transfer(
    key: &HybridSigningKey,
    nonce: u64,
    outputs: &[([u8; 32], U256)],
    chain: &ChainTag,
) -> Transaction {
    let outputs = outputs
        .iter()
        .map(|(recipient, amount)| TxOutput {
            amount: *amount,
            recipient: *recipient,
        })
        .collect();
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(key, chain).unwrap();
    tx
}

/// Writes one storage key; four lines, so a root that moves is attributable.
const WRITER_WAT: &str = r#"(module
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (i32.store8 (i32.const 0) (i32.const 107))
      (i32.store (i32.const 16) (i32.const 1234))
      (call $write (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 4))
      (i64.const 0)))"#;

/// The transactions of each block, by height. Empty blocks close the vote
/// and execute the proposal; their heights come from the proposal record.
fn scenario(chain: &ChainTag) -> Vec<(u64, Vec<Transaction>)> {
    let (a, b, c) = (alice(), bob(), carol());
    let asset = derive_asset_id(&a.address(), 1, &GOLD);
    let pair = derive_pair_id(&NATIVE_ASSET, &asset, LP_FEE_BPS);
    let code = wat::parse_str(WRITER_WAT).unwrap();
    let contract = derive_contract_id(&a.address(), 4, &code);
    let proposal = derive_proposal_id(&a.address(), 6);
    let new_validator = validator(GENESIS_VALIDATORS);
    let new_key = new_validator.verifying_key().to_bytes();
    let possession = new_validator
        .sign(&StakingAction::possession_message(&b.address()))
        .unwrap();
    let lock = |amount| {
        TxKind::LockStake(StakeLock {
            amount: coins(amount),
            unlock_height: 200_000,
        })
    };
    let vote = TxKind::CastVote(Ballot {
        proposal,
        choice: Choice::For.tag(),
    });

    vec![
        (
            1,
            vec![
                transfer(
                    &a,
                    0,
                    &[(b.address(), coins(1_250)), (c.address(), U256::from(7))],
                    chain,
                ),
                transfer(&b, 0, &[(c.address(), coins(3))], chain),
                signed(
                    TxKind::RegisterAsset(AssetRegistration {
                        symbol: GOLD,
                        total_supply: U256::from(4_000_000),
                    }),
                    1,
                    &a,
                    chain,
                ),
                signed(lock(40_000), 2, &a, chain),
                signed(lock(40_000), 0, &c, chain),
            ],
        ),
        (
            2,
            vec![
                signed(
                    TxKind::CreatePool(PoolCreation {
                        asset_a: NATIVE_ASSET,
                        asset_b: asset,
                        lp_fee_bps: LP_FEE_BPS,
                        amount_a: U256::from(100_000),
                        amount_b: U256::from(400_000),
                    }),
                    3,
                    &a,
                    chain,
                ),
                signed(
                    TxKind::DeployContract(ContractDeploy { code }),
                    4,
                    &a,
                    chain,
                ),
            ],
        ),
        (
            3,
            vec![
                signed(
                    TxKind::Swap(SwapRequest {
                        pair,
                        direction: Direction::BaseToQuote.tag(),
                        amount_in: U256::from(1_000),
                        min_out: U256::one(),
                        deadline: 0,
                    }),
                    1,
                    &b,
                    chain,
                ),
                signed(
                    TxKind::Staking(Box::new(StakingAction::Register {
                        key: Box::new(new_key),
                        bond: coins(2_000),
                        commission_bps: 300,
                        possession: Box::new(possession),
                    })),
                    2,
                    &b,
                    chain,
                ),
                signed(
                    TxKind::CallContract(ContractCall {
                        contract,
                        input: Vec::new(),
                        gas_limit: 1_000_000,
                    }),
                    5,
                    &a,
                    chain,
                ),
                signed(
                    TxKind::Staking(Box::new(StakingAction::Delegate {
                        validator: validator_id(&validator(0).verifying_key().to_bytes()),
                        amount: coins(500),
                    })),
                    1,
                    &c,
                    chain,
                ),
            ],
        ),
        (
            4,
            vec![
                signed(
                    TxKind::Propose(Box::new(ProposalSubmission {
                        voting_blocks: MIN_VOTING_BLOCKS,
                        timelock_blocks: MIN_TIMELOCK_BLOCKS,
                        upgrade: None,
                        changes: vec![(
                            ParameterKey::DexProtocolFeeBps.tag(),
                            maya_dex::Amount::from_u64(25),
                        )],
                    })),
                    6,
                    &a,
                    chain,
                ),
                signed(
                    TxKind::AddLiquidity(LiquidityDeposit {
                        pair,
                        base_desired: U256::from(500),
                        quote_desired: U256::from(1_900),
                        min_shares: U256::one(),
                    }),
                    3,
                    &b,
                    chain,
                ),
            ],
        ),
        (
            5,
            vec![
                signed(vote.clone(), 7, &a, chain),
                signed(vote, 2, &c, chain),
            ],
        ),
    ]
}

fn header(prev_hash: [u8; 32], timestamp: u64, difficulty_target: [u8; 32]) -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash,
        state_root: [0; 32],
        timestamp,
        nonce: 0,
        difficulty_target,
        tx_root: [0; 32],
    }
}

/// Seals `txs` at `height` on top of `prev`, declaring the root execution
/// produces, and applies it.
fn seal(
    db: &StateDB,
    prev: [u8; 32],
    height: u64,
    txs: Vec<Transaction>,
    target: [u8; 32],
) -> Block {
    let context = BlockContext::at_height(height);
    let mut block = Block::new(header(prev, 1_000_000 + height, target), txs);
    block.header.state_root = db.preview_root(&block, context).unwrap();
    db.apply_block_journaled(&block, &block.header.id(), context)
        .unwrap();
    block
}

/// Builds the chain from [`scenario`]: the text of `expected.txt` and of
/// `blocks.hex`.
fn build() -> (String, String) {
    let (_dir, db, genesis_block, genesis_root) = seeded();
    let genesis_id = genesis_block.header.id();
    let chain = ChainTag::from_genesis(genesis_id);
    let target = genesis_block.header.difficulty_target;
    let proposal = derive_proposal_id(&alice().address(), 6);

    let mut sealed = Vec::new();
    let mut prev = genesis_id;
    for (height, txs) in scenario(&chain) {
        let block = seal(&db, prev, height, txs, target);
        prev = block.header.id();
        sealed.push((height, block));
    }
    // The vote is in: close it, then execute it, at the heights the proposal
    // record names.
    let executable = db
        .proposal(&proposal)
        .unwrap()
        .unwrap()
        .proposal
        .schedule
        .executable_from;
    for height in [executable - MIN_TIMELOCK_BLOCKS + 1, executable] {
        let block = seal(&db, prev, height, Vec::new(), target);
        prev = block.header.id();
        sealed.push((height, block));
    }
    let state = db.proposal(&proposal).unwrap().unwrap().proposal.state;
    assert_eq!(
        state,
        ProposalState::Executed,
        "the scenario's proposal must execute"
    );

    let mut expected = String::new();
    writeln!(
        expected,
        "genesis {} {}",
        hex::encode(genesis_id),
        hex::encode(genesis_root)
    )
    .unwrap();
    let mut blocks = String::new();
    for (height, block) in &sealed {
        let (id, root) = (block.header.id(), block.header.state_root);
        writeln!(
            expected,
            "{height} {} {}",
            hex::encode(id),
            hex::encode(root)
        )
        .unwrap();
        writeln!(blocks, "{}", hex::encode(block.to_bytes())).unwrap();
    }
    (expected, blocks)
}

/// Under `MAYA_BLESS_GOLDEN=1`, rewrites both files, once per run: both
/// tests call it first, so neither reads a half-written file.
fn bless_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var_os(BLESS).is_some() {
            let (expected, blocks) = build();
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(dir().join("expected.txt"), expected).unwrap();
            std::fs::write(dir().join("blocks.hex"), blocks).unwrap();
        }
    });
}

fn read(name: &str) -> String {
    std::fs::read_to_string(dir().join(name))
        .unwrap_or_else(|e| panic!("{name} missing ({e}); run with {BLESS}=1"))
}

/// Rebuilds the chain in code: ML-DSA and SLH-DSA signing here are
/// deterministic, so the same scenario must give the same bytes on every OS.
#[test]
fn the_scenario_builds_exactly_the_committed_blocks() {
    bless_once();
    let (expected, blocks) = build();
    // Compared line by line: a CRLF checkout must not fail it, and a
    // mismatch should name the height, not dump 400 KB.
    for (i, (built, committed)) in expected
        .lines()
        .zip(read("expected.txt").lines())
        .enumerate()
    {
        assert_eq!(built, committed.trim_end(), "expected.txt line {}", i + 1);
    }
    for (i, (built, committed)) in blocks.lines().zip(read("blocks.hex").lines()).enumerate() {
        assert!(
            built == committed.trim_end(),
            "blocks.hex line {} differs",
            i + 1
        );
    }
    assert_eq!(
        expected.lines().count(),
        read("expected.txt").lines().count()
    );
    assert_eq!(blocks.lines().count(), read("blocks.hex").lines().count());
}

#[test]
fn the_committed_chain_reproduces_its_golden_ids_and_roots() {
    bless_once();
    let expected = read("expected.txt");
    let blocks = read("blocks.hex");
    let mut expected = expected
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>());
    let mut blocks = blocks.lines().filter(|l| !l.trim().is_empty());

    let (_dir, db, genesis_block, genesis_root) = seeded();
    let first = expected.next().expect("genesis line");
    assert_eq!(first[0], "genesis");
    assert_eq!(
        hex::encode(genesis_block.header.id()),
        first[1],
        "genesis id"
    );
    assert_eq!(hex::encode(genesis_root), first[2], "genesis state root");

    let mut prev = genesis_block.header.id();
    let mut applied = 0;
    for line in expected {
        let height: u64 = line[0].parse().unwrap();
        let bytes = hex::decode(blocks.next().expect("a block per line").trim()).unwrap();
        let block = Block::from_bytes(&bytes).unwrap();
        assert_eq!(block.to_bytes(), bytes, "height {height}: block re-encodes");
        assert_eq!(block.header.prev_hash, prev, "height {height}: chain link");
        let rebuilt = Block::new(block.header.clone(), block.transactions.clone());
        assert_eq!(
            rebuilt.header.tx_root, block.header.tx_root,
            "height {height}: tx root"
        );
        let id = block.header.id();
        assert_eq!(hex::encode(id), line[1], "height {height}: block id");
        // Refuses a declared root that execution does not reproduce.
        let root = db
            .apply_block_journaled(&block, &id, BlockContext::at_height(height))
            .unwrap_or_else(|e| panic!("height {height}: {e}"));
        assert_eq!(hex::encode(root), line[2], "height {height}: state root");
        prev = id;
        applied += 1;
    }
    assert!(blocks.next().is_none(), "more blocks than expected roots");
    assert_eq!(applied, 7, "five scenario blocks plus close and execute");
    assert_eq!(
        db.parameters()
            .unwrap()
            .get(ParameterKey::DexProtocolFeeBps)
            .saturating_to_u64(),
        25,
        "the replayed proposal executed"
    );
    // A losing swap and a trapped call are no-ops, not errors (invariant 7),
    // so the roots alone would not show the scenario lost its DEX or VM half.
    let a = alice().address();
    let asset = derive_asset_id(&a, 1, &GOLD);
    let code = wat::parse_str(WRITER_WAT).unwrap();
    assert_eq!(
        db.get_contract_storage(&derive_contract_id(&a, 4, &code), b"k")
            .unwrap(),
        Some(1234u32.to_le_bytes().to_vec()),
        "the contract call wrote its key"
    );
    assert!(
        db.asset_balance(&asset, &bob().address()).unwrap() > U256::zero(),
        "the swap settled"
    );
}
