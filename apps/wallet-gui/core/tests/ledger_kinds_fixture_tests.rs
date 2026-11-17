//! The wallet's half of the Ledger clear-sign contract (P-11).
//!
//! The Ledger app signs only what it can show: a v7 suite-`0x10` plain
//! transfer of 1 to 4 outputs. Everything else must be refused before a
//! screen, never shown as "a transfer" and never signed blind. This test
//! writes, with the node's own encoder, to
//! `apps/ledger-maya2c/tests/fixtures/suite-0x10-kinds.txt`, the signing
//! bytes of the shapes the wallet builds or describes, plus payload kinds the
//! device must refuse:
//!
//! - built by the wallet core: a transfer with its fee output, and a swap;
//! - described by its confirm screen: transfer, asset transfer, contract
//!   call, vault request and vault configure;
//! - refusal probes the wallet neither builds nor describes: 0 and 5 output
//!   transfers, deploy-contract, open-channel, delegate, lock-stake,
//!   cancel-order, finalize-dispute, cancel-proposal, and a v8 multisig spend;
//! - one value of every payload kind the node defines (tags 1 to 53), decoded
//!   by the node itself, as the `every-kind-NN-*` lines.
//!
//! The device clear-signs no payload kind: it shows only plain transfers of
//! 1 to 4 outputs and must refuse every kind line. The app's host tests and
//! its Speculos tests read the file and assert each verdict.
//!
//! The device key is written as `KEY`: each test puts its own key there, as
//! `ledger_tests.rs` already does for the transfer fixture.
//!
//! Compared, not rewritten, unless `MAYA_WRITE_LEDGER_FIXTURE=1` is set.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::dex_payload::{AssetTransfer, SwapRequest};
use custom_l1_node::core::governance_payload::StakeLock;
use custom_l1_node::core::multisig_tx::MultisigAuth;
use custom_l1_node::core::payload::{ChannelOpen, ContractCall, ContractDeploy, TxKind};
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::core::transaction::{Transaction, TxInput, TxOutput};
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::state::fees::FEE_COLLECTOR;
use maya_crypto_pq::multisig::{MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId};
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

/// A v8 multisig spend from a 1-of-1 policy naming the device key. The
/// device cannot approve multisig (its review reads only the v7 domain), so
/// it must refuse these bytes as not a transaction (`6a90`), never show them.
/// When a v8 review lands, this verdict flips and the wallet's multisig
/// fund-lock guard (`MultisigAccount::new`) can be relaxed with it.
fn multisig_spend(public: &[u8]) -> Transaction {
    let policy = MultisigPolicy::new(
        1,
        vec![PolicyKey {
            suite: SuiteId::MlDsa65,
            public_key: public.to_vec(),
        }],
    )
    .expect("1-of-1");
    let mut tx = frame(vec![output(9, 0x33)], TxKind::Transfer);
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy)));
    tx
}

fn fixture() -> String {
    let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(CHAIN_KEY));
    let raw = MlDsa65::public_key(&key);
    let public = hex::encode(&raw);
    let mut out = String::new();
    let mut line = |name: &str, verdict: &str, tx: &Transaction| {
        let bytes = hex::encode(tx.signing_bytes(&CHAIN));
        assert_eq!(bytes.matches(&public).count(), 1, "{name}: one key");
        let _ = writeln!(out, "{name} {verdict} {}", bytes.replace(&public, "KEY"));
    };
    for (name, verdict, mut tx) in cases() {
        tx.sign_with_suite::<MlDsa65>(&key, &CHAIN).expect("sign");
        line(name, verdict, &tx);
    }
    line("multisig-v8-spend", "6a90", &multisig_spend(&raw));
    // Every payload kind the node defines, each refused before a screen.
    for kind in every_payload_kind() {
        let tag = wire_tag(&kind).expect("a payload kind");
        let name = format!("every-kind-{tag:02}-{}", kind.label());
        let mut tx = frame(vec![output(9, 0x33)], kind);
        tx.sign_with_suite::<MlDsa65>(&key, &CHAIN).expect("sign");
        line(&name, "6a95", &tx);
    }
    out
}

/// The wire tag of every kind, written out by hand. Exhaustive on purpose: a
/// new `TxKind` variant does not compile until it is added here, and then
/// [`every_payload_kind`] and the fixture must carry it too.
fn wire_tag(kind: &TxKind) -> Option<u8> {
    let tag = match kind {
        TxKind::Transfer => return None,
        TxKind::OpenChannel(_) => 1,
        TxKind::CooperativeClose(_) => 2,
        TxKind::DisputeClose(_) => 3,
        TxKind::PenaltyClaim(_) => 4,
        TxKind::SettleBatch(_) => 5,
        TxKind::FinalizeDispute(_) => 6,
        TxKind::DeployContract(_) => 7,
        TxKind::CallContract(_) => 8,
        TxKind::Shielded(_) => 9,
        TxKind::RegisterAsset(_) => 10,
        TxKind::TransferAsset(_) => 11,
        TxKind::CreatePool(_) => 12,
        TxKind::AddLiquidity(_) => 13,
        TxKind::RemoveLiquidity(_) => 14,
        TxKind::Swap(_) => 15,
        TxKind::SwapRoute(_) => 16,
        TxKind::PlaceOrder(_) => 17,
        TxKind::CancelOrder(_) => 18,
        TxKind::CreateFeed(_) => 19,
        TxKind::SubmitFeed(_) => 20,
        TxKind::RotateAuthorities(_) => 21,
        TxKind::SubmitBeacon(_) => 22,
        TxKind::ClaimWork(_) => 23,
        TxKind::LockStake(_) => 24,
        TxKind::UnlockStake(_) => 25,
        TxKind::Propose(_) => 26,
        TxKind::CastVote(_) => 27,
        TxKind::CancelProposal(_) => 28,
        TxKind::Seal(_) => 29,
        TxKind::RevealShare(_) => 30,
        TxKind::RegisterDid(_) => 31,
        TxKind::RotateDidKey(_) => 32,
        TxKind::RevokeDid(_) => 33,
        TxKind::AnchorAttestation(_) => 34,
        TxKind::SetRevocationBit(_) => 35,
        TxKind::IssueRwa(_) => 36,
        TxKind::SettleDvp(_) => 37,
        TxKind::RecordEligibility(_) => 38,
        TxKind::DistributeRevenue(_) => 39,
        TxKind::AttestLegal(_) => 40,
        TxKind::HtlcLock(_) => 41,
        TxKind::HtlcClaim(_) => 42,
        TxKind::HtlcRefund(_) => 43,
        TxKind::AttestAttack(_) => 44,
        TxKind::EnrollDevice(_) => 45,
        TxKind::SubmitTelemetry(_) => 46,
        TxKind::ReportTamper(_) => 47,
        TxKind::ProveEquivocation(_) => 48,
        TxKind::RevokeDevice(_) => 49,
        TxKind::Staking(_) => 50,
        TxKind::Council(_) => 51,
        TxKind::Vault(_) => 52,
        TxKind::MiningShare(_) => 53,
    };
    Some(tag)
}

/// The highest payload tag `wire_tag` names. The node must refuse the next
/// one (`no_payload_tag_past_the_last_is_defined`), so a kind added to the
/// node without being named here fails a test.
const LAST_TAG: u8 = 53;

/// One value of every payload kind, as the node itself decodes it from its
/// tag and a body of zero bytes.
fn every_payload_kind() -> Vec<TxKind> {
    (1..=LAST_TAG)
        .map(|tag| {
            let bytes = payload_bytes(tag);
            let mut reader = ByteReader::new(&bytes);
            TxKind::decode(&mut reader).unwrap_or_else(|e| panic!("tag {tag}: {e}"))
        })
        .collect()
}

/// `tag` and a body the node decodes: zeros (empty lengths, zero amounts,
/// all-zero keys and hashes), with one byte set for the kinds whose decoder
/// refuses an all-zero body. Each entry is `(tag, body offset, value)`.
fn payload_bytes(tag: u8) -> Vec<u8> {
    // Room for the largest body: a channel closure carries two hybrid signatures.
    const ZEROS: usize = 65_536;
    const NOT_ALL_ZERO: [(u8, usize, u8); 8] = [
        (16, 0, 1),     // swap route: one leg
        (20, 48, 1),    // feed submission: one observation
        (26, 16, 1),    // proposal: one parameter change
        (29, 8, 48),    // sealed envelope: the minimum ciphertext
        (41, 72, 2),    // HTLC lock: a BLAKE3 hash lock, not a lattice one
        (44, 0, 1),     // attack attestation: offence 1
        (45, 1_952, 1), // device enrollment: sensor class 1, after the key
        (47, 40, 1),    // tamper report: cause 1
    ];
    let mut bytes = vec![tag];
    bytes.resize(1 + ZEROS, 0);
    for (_, offset, value) in NOT_ALL_ZERO.iter().filter(|(t, _, _)| *t == tag) {
        bytes[1 + offset] = *value;
    }
    bytes
}

#[test]
fn every_payload_kind_carries_bytes_after_the_nonce() {
    // The device refuses anything after the nonce; that is only a refusal of
    // every payload kind if no payload kind encodes to nothing. Checked for
    // one value of each of the node's kinds, not just the fixture's.
    let kinds = every_payload_kind();
    assert_eq!(kinds.len(), usize::from(LAST_TAG));
    for (tag, kind) in (1..=LAST_TAG).zip(&kinds) {
        let mut tail = Vec::new();
        kind.encode_into(&mut tail);
        assert_eq!(tail.first(), Some(&tag), "{}", kind.label());
        assert_eq!(wire_tag(kind), Some(tag), "{}", kind.label());
    }
    let mut tail = Vec::new();
    TxKind::Transfer.encode_into(&mut tail);
    assert!(tail.is_empty(), "a plain transfer carries no payload section");
    for (name, _, tx) in cases() {
        assert_eq!(wire_tag(&tx.kind).is_none(), matches!(tx.kind, TxKind::Transfer), "{name}");
    }
}

#[test]
fn no_payload_tag_past_the_last_is_defined() {
    let bytes = payload_bytes(LAST_TAG + 1);
    assert!(TxKind::decode(&mut ByteReader::new(&bytes)).is_err());
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
