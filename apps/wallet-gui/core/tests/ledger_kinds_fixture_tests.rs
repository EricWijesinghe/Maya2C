//! The wallet's half of the Ledger clear-sign contract (P-11).
//!
//! The Ledger app signs only what it can show: a v7 suite-`0x10` plain
//! transfer of 1 to 4 outputs. Everything else must be refused before a
//! screen, never shown as "a transfer" and never signed blind. This test
//! writes, with the node's own encoder, the signing bytes of every shape the
//! wallet builds or its confirm screen describes, to
//! `apps/ledger-maya2c/tests/fixtures/suite-0x10-kinds.txt`. The app's host
//! tests and its Speculos tests read that file and assert each verdict.
//!
//! The device key is written as `KEY`: each test puts its own key there, as
//! `ledger_tests.rs` already does for the transfer fixture.
//!
//! Compared, not rewritten, unless `MAYA_WRITE_LEDGER_FIXTURE=1` is set.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::dex_payload::{AssetTransfer, SwapRequest};
use custom_l1_node::core::governance_payload::StakeLock;
use custom_l1_node::core::payload::{ChannelOpen, ContractCall, ContractDeploy, TxKind};
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::core::transaction::{Transaction, TxInput, TxOutput};
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::state::fees::FEE_COLLECTOR;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use primitive_types::U256;

/// The same genesis and chain key as `suite-0x10-transfer.txt`.
const CHAIN: ChainTag = ChainTag::from_genesis([0x5a; 32]);
const CHAIN_KEY: [u8; 32] = [0x11; 32];

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ledger-maya2c/tests/fixtures/suite-0x10-kinds.txt")
}

fn output(amount: u64, to: u8) -> TxOutput {
    TxOutput {
        amount: U256::from(amount),
        recipient: [to; 32],
    }
}

/// One input, `outputs`, nonce 7: the transfer fixture's frame.
fn frame(outputs: Vec<TxOutput>, kind: TxKind) -> Transaction {
    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [0x22; 32],
            index: 1,
        }],
        outputs,
        7,
    );
    tx.kind = kind;
    tx
}

/// `name`, the verdict the device must reach (`ok` or a status word), and the
/// transaction.
fn cases() -> Vec<(&'static str, &'static str, Transaction)> {
    let a = [0x41; 32];
    let two = || vec![output(1_250_000, 0x33), output(40_000, 0x44)];
    let kind = |k| frame(two(), k);
    vec![
        (
            "transfer-1-output",
            "ok",
            frame(vec![output(9, 0x33)], TxKind::Transfer),
        ),
        (
            // What `Wallet::sign` builds: the payment, then the fee to the collector.
            "transfer-wallet-shape",
            "ok",
            frame(
                vec![
                    output(500, 0x33),
                    TxOutput {
                        amount: U256::from(21),
                        recipient: FEE_COLLECTOR,
                    },
                ],
                TxKind::Transfer,
            ),
        ),
        (
            "transfer-4-outputs",
            "ok",
            frame(
                (1..=4u8).map(|i| output(u64::from(i), 0x30 + i)).collect(),
                TxKind::Transfer,
            ),
        ),
        (
            "transfer-5-outputs",
            "6a92",
            frame(
                (1..=5u8).map(|i| output(u64::from(i), 0x30 + i)).collect(),
                TxKind::Transfer,
            ),
        ),
        (
            "transfer-0-outputs",
            "6a92",
            frame(vec![], TxKind::Transfer),
        ),
        (
            "kind-swap",
            "6a95",
            kind(TxKind::Swap(SwapRequest {
                pair: [0x51; 32],
                direction: 0,
                amount_in: U256::from(1_000),
                min_out: U256::from(990),
                deadline: 0,
            })),
        ),
        (
            "kind-transfer-asset",
            "6a95",
            kind(TxKind::TransferAsset(AssetTransfer {
                asset: [0x10; 32],
                recipient: a,
                amount: U256::from(7),
            })),
        ),
        (
            "kind-call-contract",
            "6a95",
            kind(TxKind::CallContract(ContractCall {
                contract: a,
                input: vec![1, 2],
                gas_limit: 9,
            })),
        ),
        (
            "kind-deploy-contract",
            "6a95",
            kind(TxKind::DeployContract(ContractDeploy {
                code: b"\0asm\x01\0\0\0".to_vec(),
            })),
        ),
        (
            "kind-open-channel",
            "6a95",
            kind(TxKind::OpenChannel(ChannelOpen {
                counterparty: a,
                funding: U256::from(5),
                dispute_window: 100,
            })),
        ),
        (
            "kind-delegate",
            "6a95",
            kind(TxKind::Staking(Box::new(StakingAction::Delegate {
                validator: a,
                amount: U256::from(3),
            }))),
        ),
        (
            "kind-lock-stake",
            "6a95",
            kind(TxKind::LockStake(StakeLock {
                amount: U256::from(1),
                unlock_height: 2,
            })),
        ),
        (
            "kind-vault-request",
            "6a95",
            kind(TxKind::Vault(Box::new(VaultAction::Request {
                to: a,
                amount: U256::from(3),
            }))),
        ),
        (
            "kind-vault-configure",
            "6a95",
            kind(TxKind::Vault(Box::new(VaultAction::Configure(
                VaultConfig {
                    delay_blocks: 3_600,
                    limit: U256::from(10),
                    guardians: vec![a],
                },
            )))),
        ),
        (
            "kind-cancel-order",
            "6a95",
            kind(TxKind::CancelOrder([4; 32])),
        ),
        (
            "kind-finalize-dispute",
            "6a95",
            kind(TxKind::FinalizeDispute([5; 32])),
        ),
        (
            "kind-cancel-proposal",
            "6a95",
            kind(TxKind::CancelProposal([6; 32])),
        ),
    ]
}

fn fixture() -> String {
    let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(CHAIN_KEY));
    let public = hex::encode(MlDsa65::public_key(&key));
    let mut out = String::new();
    for (name, verdict, mut tx) in cases() {
        tx.sign_with_suite::<MlDsa65>(&key, &CHAIN).expect("sign");
        let bytes = hex::encode(tx.signing_bytes(&CHAIN));
        assert_eq!(bytes.matches(&public).count(), 1, "{name}: one key");
        let _ = writeln!(out, "{name} {verdict} {}", bytes.replace(&public, "KEY"));
    }
    out
}

#[test]
fn every_payload_kind_carries_bytes_after_the_nonce() {
    // The device refuses anything after the nonce; that is only a refusal of
    // every payload kind if no payload kind encodes to nothing.
    for (name, _, tx) in cases() {
        let mut tail = Vec::new();
        tx.kind.encode_into(&mut tail);
        assert_eq!(
            tail.is_empty(),
            matches!(tx.kind, TxKind::Transfer),
            "{name}"
        );
    }
}

#[test]
fn the_ledger_kinds_fixture_is_what_the_node_computes() {
    let expected = fixture();
    let path = fixture_path();
    if std::env::var("MAYA_WRITE_LEDGER_FIXTURE").as_deref() == Ok("1") {
        std::fs::write(&path, &expected).expect("write fixture");
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (set MAYA_WRITE_LEDGER_FIXTURE=1)", path.display()));
    assert!(
        on_disk.replace("\r\n", "\n") == expected,
        "the node no longer produces the Ledger kinds fixture; regenerate it with \
         MAYA_WRITE_LEDGER_FIXTURE=1 and rerun apps/ledger-maya2c's tests"
    );
}
