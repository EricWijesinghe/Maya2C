//! Two decode gaps that let a record exist on chain and fail to leave it.
//!
//! Found while building lattice HTLCs, 2026-09-13:
//!
//! 1. `TxKind::decode` had no arms for the RWA tags 36–40. An RWA transaction
//!    encoded, signed and executed, and then could not be read back from its
//!    own bytes — so not submitted over RPC, not gossiped, not loaded from a
//!    stored block.
//! 2. `StateLayer::from_tag` had no arms for Identity (9) and RWA (10), and
//!    `LAYER_ORDER` omitted both. An `AccountProof` from a state holding either
//!    kind of record failed to decode, and failed to verify if decoding were
//!    skipped.

use custom_l1_node::core::htlc_payload::HtlcLock;
use custom_l1_node::core::rwa_payload::{
    AttestLegal, DistributeRevenue, IssueRwa, RecordEligibility, RulePayload, SettleDvp,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::proof::{LAYER_ORDER, StateLayer};
use custom_l1_node::state::{Account, AccountProof, BlockContext, StateDB};

use maya_htlc_lattice::LatticeSecret;
use tempfile::TempDir;

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key).expect("sign");
    tx
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_789_200_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

#[test]
fn every_rwa_kind_decodes_from_its_own_bytes() {
    let key = generate_signing_key().expect("keygen");
    let kinds = [
        TxKind::IssueRwa(Box::new(IssueRwa {
            label: "Building A".to_owned(),
            total_units: 1_000,
            rule: Some(RulePayload {
                schema: [1; 32],
                predicate_tag: 0,
                bound: 18,
                eligibility_blocks: 100,
                trusted_issuers: vec![[2; 32]],
            }),
        })),
        TxKind::SettleDvp(Box::new(SettleDvp {
            asset: [3; 32],
            seller: [4; 32],
            units: 5,
            price: 6,
            page: 7,
        })),
        TxKind::RecordEligibility(RecordEligibility { asset: [8; 32] }),
        TxKind::DistributeRevenue(Box::new(DistributeRevenue {
            asset: [9; 32],
            round: 1,
            total: 2,
            pages: 3,
        })),
        TxKind::AttestLegal(Box::new(AttestLegal {
            asset: [10; 32],
            document: [11; 32],
            reference: "ipfs://doc".to_owned(),
        })),
    ];
    for kind in kinds {
        let tx = signed(kind, 0, &key);
        let decoded = Transaction::from_bytes(&tx.to_bytes())
            .unwrap_or_else(|e| panic!("{}: {e}", tx.kind.label()));
        assert_eq!(decoded.kind, tx.kind);
        assert_eq!(decoded.txid(), tx.txid());
    }
}

#[test]
fn every_layer_in_the_fold_order_round_trips_its_tag() {
    for layer in LAYER_ORDER {
        assert_eq!(StateLayer::from_tag(layer.tag()), Some(*layer), "{layer:?}");
    }
    for layer in [StateLayer::Identity, StateLayer::Rwa, StateLayer::Htlc] {
        assert!(LAYER_ORDER.contains(&layer), "{layer:?} missing from LAYER_ORDER");
    }
}

#[test]
fn an_account_proof_verifies_from_a_state_holding_rwa_and_htlc_records() {
    let dir = TempDir::new().expect("dir");
    let db = StateDB::open(dir.path()).expect("open");
    let key = generate_signing_key().expect("keygen");
    let address = key.address();
    db.put_account(
        &address,
        &Account {
            balance: 100_000,
            nonce: 0,
        },
    )
    .expect("fund");

    let issue = TxKind::IssueRwa(Box::new(IssueRwa {
        label: "Building B".to_owned(),
        total_units: 10,
        rule: None,
    }));
    let lock = TxKind::HtlcLock(Box::new(HtlcLock {
        recipient: [7; 32],
        amount: 1_000,
        expiry_height: 100,
        commitment: LatticeSecret::from_entropy([1; 32])
            .commitment()
            .expect("commit"),
    }));
    let context = BlockContext::at_height(1).with_htlc_activation(0);
    db.apply_block(&block_of(vec![signed(issue, 0, &key)]), context)
        .expect("issue");
    db.apply_block(&block_of(vec![signed(lock, 1, &key)]), context)
        .expect("lock");

    let proof = db
        .account_proof(&address)
        .expect("read")
        .expect("account exists");
    let layers: Vec<StateLayer> = proof.layers.iter().map(|digest| digest.layer).collect();
    assert!(layers.contains(&StateLayer::Rwa) && layers.contains(&StateLayer::Htlc));

    let decoded = AccountProof::decode(&proof.encode()).expect("decodes");
    assert!(decoded.verify(&db.state_root().expect("root")).expect("verifies"));
}
