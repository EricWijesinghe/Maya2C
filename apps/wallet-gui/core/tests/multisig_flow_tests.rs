//! The wallet's multisig flow (P-16): a mixed-suite 2-of-3 account (two
//! hybrid `0x30` wallet keys and an ML-DSA-65 key, the suite the Ledger app
//! signs with), a proposal passed between members as hex, and the node
//! accepting the finished spend.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{ChainTag, Transaction};
use custom_l1_node::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT;
use maya_clear_sign::Context;
use maya_crypto_pq::agility::{Network, SuitePolicy};
use maya_crypto_pq::multisig::Approval;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId};
use maya_wallet_core::error::WalletError;
use maya_wallet_core::multisig::{MemberKey, MultisigAccount, Proposal};
use maya_wallet_core::wallet::{SignTerms, Wallet};
use primitive_types::U256;

const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);
const OTHER_CHAIN: ChainTag = ChainTag::from_genesis([0x5B; 32]);
const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon about";
const RECIPIENT: &str = "3333333333333333333333333333333333333333333333333333333333333333";

type LedgerKey = <MlDsa65 as SignatureSuite>::SigningKey;

fn wallet() -> Wallet {
    Wallet::from_mnemonic(PHRASE, "").expect("phrase")
}

/// The Ledger app's suite: ML-DSA-65 from a 32-byte chain key.
fn ledger_key() -> LedgerKey {
    MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([0x11; 32]))
}

fn ledger_member() -> MemberKey {
    MemberKey {
        suite: SuiteId::MlDsa65.to_byte(),
        public_key: hex::encode(MlDsa65::public_key(&ledger_key())),
    }
}

/// Wallet accounts 0 and 1, then the Ledger key: 2 of 3.
fn account(w: &Wallet) -> MultisigAccount {
    let members = [
        w.member_key(0).expect("0"),
        w.member_key(1).expect("1"),
        ledger_member(),
    ];
    MultisigAccount::new(2, &members).expect("2-of-3")
}

fn terms(fee: U256) -> SignTerms<'static> {
    SignTerms {
        fee,
        fee_to: None,
        nonce: 4,
        chain: &CHAIN,
        allow_high_fee: false,
    }
}

fn propose(w: &Wallet) -> Proposal {
    w.propose_multisig(
        0,
        &account(w),
        RECIPIENT,
        U256::from(500),
        &terms(U256::zero()),
    )
    .expect("propose")
}

/// Account `index` reviews and approves `p`.
fn approve(w: &mut Wallet, index: u32, p: &Proposal) -> Proposal {
    let shown = w.confirm_multisig(p, &CHAIN, &Context::default());
    w.approve_multisig(index, p, &CHAIN, &shown.signing_hash)
        .expect("approve")
}

fn ledger_approval(p: &Proposal, index: u8) -> Approval {
    Approval {
        index,
        signature: MlDsa65::sign(&ledger_key(), &p.signing_bytes(&CHAIN)).expect("sign"),
    }
}

#[test]
fn a_mixed_suite_quorum_produces_a_spend_the_node_accepts() {
    let mut w = wallet();
    let account = account(&w);
    let suites: Vec<u8> = account.members().iter().map(|m| m.suite).collect();
    assert_eq!(suites, [0x30, 0x30, 0x10], "hybrid, hybrid, ML-DSA-65");

    let draft = propose(&w);
    let first = approve(&mut w, 0, &draft);
    assert!(!first.is_complete());

    // The second member gets only the hex, as from another machine.
    let shared = Proposal::from_hex(&first.to_hex(), &CHAIN).expect("read back");
    assert_eq!(shared, first);
    let done = shared
        .with_approval(ledger_approval(&shared, 2), &CHAIN)
        .expect("ledger approval");
    assert!(done.is_complete());
    assert_eq!(done.approved_by(), [0, 2]);

    let signed = done.finish(&CHAIN).expect("finish");
    assert_eq!(signed.sender, hex::encode(account.address()));
    assert_eq!(signed.recipient, RECIPIENT);
    assert_eq!(signed.amount, U256::from(500));

    let tx = Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("v8");
    let genesis = SuitePolicy::genesis(Network::Mainnet);
    assert_eq!(
        tx.verify_at(SUITE_ENVELOPE_ACTIVATION_HEIGHT, &genesis, &CHAIN),
        Ok(())
    );
    assert_eq!(tx.sender(), account.address());
    // Bound to its chain (ADR-036).
    assert!(
        tx.verify_at(SUITE_ENVELOPE_ACTIVATION_HEIGHT, &genesis, &OTHER_CHAIN)
            .is_err()
    );
}

#[test]
fn members_can_approve_in_parallel_and_merge() {
    let mut w = wallet();
    let draft = propose(&w);
    let a = approve(&mut w, 1, &draft);
    let b = draft
        .with_approval(ledger_approval(&draft, 2), &CHAIN)
        .expect("b");
    let merged = a.merge(&b, &CHAIN).expect("merge");
    assert_eq!(merged.approved_by(), [1, 2]);
    assert!(merged.finish(&CHAIN).is_ok());
    assert_eq!(b.merge(&a, &CHAIN).expect("other way"), merged);
}

#[test]
fn the_account_reads_back_from_its_hex_as_the_same_address() {
    let w = wallet();
    let account = account(&w);
    let back = MultisigAccount::from_hex(&account.to_hex()).expect("read");
    assert_eq!(back, account);
    assert_eq!(back.address(), account.address());
    assert!(MultisigAccount::from_hex(&format!("{}00", account.to_hex())).is_err());
}

#[test]
fn an_unmeetable_repeated_or_classical_policy_is_refused() {
    let w = wallet();
    let me = w.member_key(0).expect("0");
    let other = w.member_key(1).expect("1");
    for (threshold, members) in [
        (0, vec![me.clone(), other.clone()]),
        (3, vec![me.clone(), other.clone()]),
        (2, vec![me.clone(), me.clone()]),
    ] {
        assert!(matches!(
            MultisigAccount::new(threshold, &members),
            Err(WalletError::Multisig(_))
        ));
    }
    let classical = MemberKey {
        suite: SuiteId::Ed25519.to_byte(),
        public_key: "11".repeat(32),
    };
    assert!(MultisigAccount::new(1, &[me.clone(), classical]).is_err());
    let short = MemberKey {
        suite: SuiteId::MlDsa65.to_byte(),
        public_key: "11".repeat(32),
    };
    assert!(MultisigAccount::new(1, &[me, short]).is_err());
}

#[test]
fn approving_needs_this_sessions_confirm_screen() {
    let mut w = wallet();
    let p = propose(&w);
    let hash = w
        .confirm_multisig(&p, &CHAIN, &Context::default())
        .signing_hash;
    // A hash for another proposal, or none at all, signs nothing.
    assert!(matches!(
        w.approve_multisig(0, &p, &CHAIN, &"00".repeat(32)),
        Err(WalletError::Signing(_))
    ));
    let approved = w.approve_multisig(0, &p, &CHAIN, &hash).expect("approve");
    // The review is spent by the approval.
    assert!(matches!(
        w.approve_multisig(1, &p, &CHAIN, &hash),
        Err(WalletError::Signing(_))
    ));
    assert_eq!(approved.approved_by(), [0]);
}

#[test]
fn a_non_member_or_a_repeat_approval_is_refused() {
    let mut w = wallet();
    let draft = propose(&w);
    let p = approve(&mut w, 0, &draft);
    let shown = w.confirm_multisig(&p, &CHAIN, &Context::default());
    assert!(matches!(
        w.approve_multisig(5, &p, &CHAIN, &shown.signing_hash),
        Err(WalletError::Multisig(_))
    ));
    let shown = w.confirm_multisig(&p, &CHAIN, &Context::default());
    assert!(matches!(
        w.approve_multisig(0, &p, &CHAIN, &shown.signing_hash),
        Err(WalletError::Multisig(_))
    ));
}

#[test]
fn a_forged_or_misplaced_approval_is_refused() {
    let w = wallet();
    let p = propose(&w);
    // The Ledger's signature under member 1's index.
    assert!(p.with_approval(ledger_approval(&p, 1), &CHAIN).is_err());
    // A signature over another chain's bytes.
    let other = Approval {
        index: 2,
        signature: MlDsa65::sign(&ledger_key(), &p.signing_bytes(&OTHER_CHAIN)).expect("sign"),
    };
    assert!(p.with_approval(other, &CHAIN).is_err());
    // An index past the policy.
    assert!(p.with_approval(ledger_approval(&p, 7), &CHAIN).is_err());
}

#[test]
fn a_tampered_proposal_is_refused_when_read() {
    let mut w = wallet();
    let draft = propose(&w);
    let p = approve(&mut w, 0, &draft);
    let mut bytes = hex::decode(p.to_hex()).expect("hex");
    // Flip a byte inside the hybrid approval's signature (the tail of the
    // frame is: approval, then the one-byte payload flag).
    let at = bytes.len() - 100;
    bytes[at] ^= 1;
    assert!(Proposal::from_hex(&hex::encode(&bytes), &CHAIN).is_err());
    // The same approval read against another chain does not verify either.
    assert!(Proposal::from_hex(&p.to_hex(), &OTHER_CHAIN).is_err());
}

#[test]
fn a_quorum_cannot_be_exceeded_or_finished_short() {
    let mut w = wallet();
    let draft = propose(&w);
    let one = approve(&mut w, 0, &draft);
    assert!(one.finish(&CHAIN).is_err(), "1 of 2 is not a spend");
    let two = approve(&mut w, 1, &one);
    // A spare approval would give one spend two encodings.
    assert!(two.with_approval(ledger_approval(&two, 2), &CHAIN).is_err());
    assert!(two.finish(&CHAIN).is_ok());
}

#[test]
fn a_proposal_with_an_unshown_output_is_refused() {
    let w = wallet();
    let p = propose(&w);
    let mut tx = Transaction::from_bytes(&hex::decode(p.to_hex()).expect("hex")).expect("v8");
    // A second output to someone other than the fee collector.
    tx.outputs
        .push(custom_l1_node::core::transaction::TxOutput {
            amount: U256::from(1_000_000),
            recipient: [0x77; 32],
        });
    assert!(matches!(
        Proposal::from_hex(&hex::encode(tx.to_bytes()), &CHAIN),
        Err(WalletError::Multisig(_))
    ));
}

#[test]
fn a_proposal_is_capped_like_a_transfer() {
    let w = wallet();
    // A fee larger than the amount is refused before anything is built.
    assert!(
        w.propose_multisig(
            0,
            &account(&w),
            RECIPIENT,
            U256::from(5),
            &terms(U256::from(6))
        )
        .is_err()
    );
}
