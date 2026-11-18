//! The wallet's multisig flow (P-16): a mixed-suite 2-of-3 account (two
//! hybrid `0x30` wallet keys and an ML-DSA-65 key), a proposal passed between
//! members as hex, and a finished spend that passes the node's `verify_at`
//! (signature, format and chain binding; not applied against state).
//!
//! The ML-DSA-65 approvals here are signed on the host. The Ledger app cannot
//! sign them today: its review accepts only the v7 transfer domain and refuses
//! v8 multisig bytes as not a transaction. So no account may need more
//! approvals than it has hybrid members (the fund-lock test below).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::multisig_tx::MultisigAuth;
use custom_l1_node::core::payload::{ContractCall, TxKind};
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::core::transaction::{TxInput, TxOutput};
use custom_l1_node::core::{ChainTag, Transaction};
use custom_l1_node::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use maya_clear_sign::Context;
use maya_crypto_pq::agility::{Network, SuitePolicy};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy};
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId};
use maya_wallet_core::error::WalletError;
use maya_wallet_core::multisig::{MemberKey, MultisigAccount, Proposal, ProposalStatus};
use maya_wallet_core::payment::FEE_SINK;
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

/// An ML-DSA-65 key (the Ledger app's suite) signing on the host, not on a
/// device: the device cannot sign v8 bytes yet.
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
        U256::zero(),
    )
    .expect("propose")
}

/// Account `index` reviews and approves `p` on a chain with fees off.
fn approve(w: &mut Wallet, index: u32, p: &Proposal) -> Proposal {
    approve_at(w, index, p, U256::zero()).expect("approve")
}

fn approve_at(
    w: &mut Wallet,
    index: u32,
    p: &Proposal,
    base_fee: U256,
) -> Result<Proposal, WalletError> {
    let shown = w.confirm_multisig(index, &p.account(), p, &CHAIN, &Context::default())?;
    w.approve_multisig(index, p, &CHAIN, &shown.signing_hash, base_fee)
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
        .confirm_multisig(0, &account(&w), &p, &CHAIN, &Context::default())
        .expect("member 0")
        .signing_hash;
    // A hash for another proposal, or none at all, signs nothing.
    assert!(matches!(
        w.approve_multisig(0, &p, &CHAIN, &"00".repeat(32), U256::zero()),
        Err(WalletError::Signing(_))
    ));
    let approved = w
        .approve_multisig(0, &p, &CHAIN, &hash, U256::zero())
        .expect("approve");
    // The review is spent by the approval.
    assert!(matches!(
        w.approve_multisig(1, &p, &CHAIN, &hash, U256::zero()),
        Err(WalletError::Signing(_))
    ));
    assert_eq!(approved.approved_by(), [0]);
}

#[test]
fn a_non_member_or_a_repeat_approval_is_refused() {
    let mut w = wallet();
    let draft = propose(&w);
    let p = approve(&mut w, 0, &draft);
    assert!(matches!(
        approve_at(&mut w, 5, &p, U256::zero()),
        Err(WalletError::Multisig(_))
    ));
    assert!(matches!(
        approve_at(&mut w, 0, &p, U256::zero()),
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
            &terms(U256::from(6)),
            U256::zero(),
        )
        .is_err()
    );
}

#[test]
fn a_policy_needing_an_approval_this_wallet_cannot_give_is_refused() {
    // Only a hybrid 0x30 key can approve today. A 2-of-2 of this wallet and
    // a Ledger would hold funds that nobody can ever spend.
    let w = wallet();
    let me = w.member_key(0).expect("0");
    assert!(matches!(
        MultisigAccount::new(2, &[me.clone(), ledger_member()]),
        Err(WalletError::Multisig(_))
    ));
    // As many approvals as hybrid members is spendable; other suites may sit
    // in the policy as spare members.
    assert!(MultisigAccount::new(1, &[me, ledger_member()]).is_ok());
}

#[test]
fn a_proposal_carrying_more_approvals_than_the_threshold_is_refused_when_read() {
    // The node decodes up to n approvals; a copy with more than m would stick
    // (finish refuses it, approvals keep adding), so a member could grief.
    let mut w = wallet();
    let draft = propose(&w);
    let one = approve(&mut w, 0, &draft);
    let two = approve(&mut w, 1, &one);
    let auth = two.transaction().multisig.as_deref().expect("v8");
    let mut approvals = auth.approvals().to_vec();
    approvals.push(ledger_approval(&two, 2));
    let mut tx = two.transaction().clone();
    tx.multisig = Some(Box::new(
        MultisigAuth::new(auth.policy().clone(), approvals).expect("shape"),
    ));
    assert!(matches!(
        Proposal::from_hex(&hex::encode(tx.to_bytes()), &CHAIN),
        Err(WalletError::Multisig(_))
    ));
}

/// A spend paying `fee` to the collector, proposed on a chain whose base fee
/// is `base_fee` per byte.
fn propose_paying(w: &Wallet, fee: u64, base_fee: u64) -> Result<Proposal, WalletError> {
    let collector = hex::encode(FEE_COLLECTOR);
    let terms = SignTerms {
        fee: U256::from(fee),
        fee_to: Some(&collector),
        nonce: 4,
        chain: &CHAIN,
        allow_high_fee: false,
    };
    w.propose_multisig(
        0,
        &account(w),
        RECIPIENT,
        U256::from(1_000_000),
        &terms,
        U256::from(base_fee),
    )
}

#[test]
fn the_fee_is_sized_for_the_finished_v8_frame() {
    let mut w = wallet();
    let size = account(&w).spend_size().expect("size");
    // A real quorum's finished frame is exactly the estimate.
    let draft = propose_paying(&w, size, 1).expect("exactly enough");
    let one = approve_at(&mut w, 0, &draft, U256::one()).expect("0");
    let done = approve_at(&mut w, 1, &one, U256::one()).expect("1");
    let raw = done.finish(&CHAIN).expect("finish").raw_hex;
    assert_eq!(u64::try_from(raw.len() / 2).expect("len"), size);
    // A single-signature transfer, which the send screen prices, is smaller.
    let transfer = w
        .transfer_size(0, &hex::encode(FEE_COLLECTOR), 4, &CHAIN)
        .expect("transfer");
    assert!(size > transfer, "{size} vs {transfer}");
    // One base unit short is refused when proposed and when approved, before
    // anyone signs a spend the node would refuse as underpaid.
    assert!(matches!(
        propose_paying(&w, size - 1, 1),
        Err(WalletError::Multisig(_))
    ));
    let short = propose_paying(&w, size - 1, 0).expect("fees off");
    assert!(matches!(
        approve_at(&mut w, 0, &short, U256::one()),
        Err(WalletError::Multisig(_))
    ));
}

#[test]
fn a_read_proposal_reports_its_own_progress() {
    let mut w = wallet();
    let draft = propose(&w);
    assert!(!draft.status().complete);
    let one = approve(&mut w, 0, &draft);
    let two = approve(&mut w, 1, &one);
    let read = Proposal::from_hex(&two.to_hex(), &CHAIN).expect("read");
    assert_eq!(
        read.status(),
        ProposalStatus {
            proposal_hex: two.to_hex(),
            approved_by: vec![0, 1],
            threshold: 2,
            complete: true,
        }
    );
}

/// A 2-of-2 of a wallet key and a Ledger key, encoded as another tool would
/// share it: `MultisigAccount::new` refuses to build it, so only an outside
/// encoder can produce the text.
fn ledger_bound_policy_hex(w: &Wallet) -> String {
    let keys = [w.member_key(0).expect("0"), ledger_member()]
        .iter()
        .map(|m| m.to_policy_key().expect("key"))
        .collect();
    hex::encode(MultisigPolicy::new(2, keys).expect("valid policy").encode())
}

#[test]
fn an_account_shared_from_elsewhere_that_no_wallet_quorum_can_spend_is_refused() {
    let w = wallet();
    assert!(matches!(
        MultisigAccount::from_hex(&ledger_bound_policy_hex(&w)),
        Err(WalletError::Multisig(_))
    ));
}

#[test]
fn the_account_card_marks_this_wallets_members_and_flags_a_stranger() {
    const GENESIS: &str = "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a";
    let mut w = wallet();
    w.account(0).expect("derive 0");
    w.account(1).expect("derive 1");
    let card = w.multisig_view(&account(&w), GENESIS).expect("view");
    let marks: Vec<Option<u32>> = card.members.iter().map(|m| m.account).collect();
    assert_eq!(marks, [Some(0), Some(1), None]);
    assert!(card.holds_member);
    // Fingerprints are BLAKE3 of the whole key, 16 bytes, not a key prefix.
    let ledger = &card.members[2];
    let key = hex::decode(&ledger.public_key).expect("hex");
    assert_eq!(
        ledger.fingerprint,
        hex::encode(&blake3::hash(&key).as_bytes()[..16])
    );
    assert_eq!(
        ledger.public_key,
        ledger_member().public_key,
        "the full key"
    );

    // Another wallet's account, opened here: nothing is marked, and the card
    // says this wallet can never approve.
    let mut stranger = Wallet::from_mnemonic(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
        "",
    )
    .expect("phrase");
    stranger.account(0).expect("derive");
    let theirs = MultisigAccount::new(1, &[stranger.member_key(0).expect("0"), ledger_member()])
        .expect("1-of-2");
    let opened = MultisigAccount::from_hex(&theirs.to_hex()).expect("read");
    let card = w.multisig_view(&opened, GENESIS).expect("view");
    assert!(!card.holds_member);
    assert!(card.members.iter().all(|m| m.account.is_none()));
}

#[test]
fn every_refusal_after_the_dialog_is_known_before_it() {
    // The Tauri command runs this before the native Approve dialog, so a
    // person is never asked to approve what the core will not sign.
    let mut w = wallet();
    let draft = propose(&w);
    assert!(w.check_multisig_approval(0, &draft, U256::zero()).is_ok());
    assert!(matches!(
        w.check_multisig_approval(5, &draft, U256::zero()),
        Err(WalletError::Multisig(_))
    ));
    let one = approve(&mut w, 0, &draft);
    assert!(matches!(
        w.check_multisig_approval(0, &one, U256::zero()),
        Err(WalletError::Multisig(_))
    ));
    let two = approve(&mut w, 1, &one);
    // Both have approved, and nobody may add to a met quorum.
    for index in [0, 1] {
        assert!(matches!(
            w.check_multisig_approval(index, &two, U256::zero()),
            Err(WalletError::Multisig(_))
        ));
    }
    let short = propose_paying(&w, 1, 0).expect("fees off");
    assert!(matches!(
        w.check_multisig_approval(0, &short, U256::one()),
        Err(WalletError::Multisig(_))
    ));
}

/// `p`'s transaction after `edit`, re-encoded as a member could send it.
fn reshaped(p: &Proposal, edit: impl FnOnce(&mut Transaction)) -> String {
    let mut tx = Transaction::from_bytes(&hex::decode(p.to_hex()).expect("hex")).expect("v8");
    edit(&mut tx);
    hex::encode(tx.to_bytes())
}

fn refused(text: &str) -> bool {
    matches!(
        Proposal::from_hex(text, &CHAIN),
        Err(WalletError::Multisig(_))
    )
}

#[test]
fn a_proposal_of_another_kind_is_refused_when_read() {
    let w = wallet();
    let p = propose(&w);
    let call = reshaped(&p, |tx| {
        tx.kind = TxKind::CallContract(ContractCall {
            contract: [0x41; 32],
            input: vec![1, 2],
            gas_limit: 9,
        });
    });
    assert!(refused(&call), "a contract call from the shared account");
    let delegate = reshaped(&p, |tx| {
        tx.kind = TxKind::Staking(Box::new(StakingAction::Delegate {
            validator: [0x41; 32],
            amount: U256::from(3),
        }));
    });
    assert!(
        refused(&delegate),
        "a delegation of the shared account's funds"
    );
}

#[test]
fn a_proposal_spending_an_input_is_refused_when_read() {
    let w = wallet();
    let p = propose(&w);
    let with_input = reshaped(&p, |tx| {
        tx.inputs.push(TxInput {
            prev_tx: [0x22; 32],
            index: 1,
        });
    });
    assert!(refused(&with_input));
}

#[test]
fn a_proposal_with_two_fee_outputs_is_refused_when_read() {
    // Each extra output alone is a fee output; three outputs are still more
    // than the one payment and one fee the screen shows.
    let w = wallet();
    let p = propose(&w);
    let fee = |recipient| TxOutput {
        amount: U256::from(10),
        recipient,
    };
    let text = reshaped(&p, |tx| {
        tx.outputs.truncate(1);
        tx.outputs.push(fee(FEE_COLLECTOR));
        tx.outputs.push(fee(FEE_SINK));
    });
    assert!(refused(&text));
}

#[test]
fn a_single_signature_frame_is_not_read_as_a_proposal() {
    // A v7 frame: the same transfer, signed by one key, no multisig section.
    let w = wallet();
    let p = propose(&w);
    let text = reshaped(&p, |tx| {
        tx.multisig = None;
        tx.sign_with_suite::<MlDsa65>(&ledger_key(), &CHAIN)
            .expect("sign");
    });
    assert!(refused(&text));
}

/// A spare SLH-DSA-SHAKE-256f member (a cold-vault key): it may sit in the
/// policy, but cannot approve today, so it never adds a signature.
fn cold_vault_member() -> MemberKey {
    MemberKey {
        suite: SuiteId::SlhDsaShake256f.to_byte(),
        public_key: "11".repeat(SuiteId::SlhDsaShake256f.info().public_key_len),
    }
}

#[test]
fn a_spare_member_with_a_longer_signature_does_not_inflate_the_fee() {
    // 2 of {0x30, 0x30, 0x21}: only the two hybrid members can approve, so
    // the finished frame carries two hybrid signatures, never the 0x21 one.
    let mut w = wallet();
    let members = [
        w.member_key(0).expect("0"),
        w.member_key(1).expect("1"),
        cold_vault_member(),
    ];
    let account = MultisigAccount::new(2, &members).expect("2-of-3");
    let size = account.spend_size().expect("size");
    let collector = hex::encode(FEE_COLLECTOR);
    let terms = SignTerms {
        fee: U256::from(size),
        fee_to: Some(&collector),
        nonce: 4,
        chain: &CHAIN,
        allow_high_fee: false,
    };
    let draft = w
        .propose_multisig(
            0,
            &account,
            RECIPIENT,
            U256::from(1_000_000),
            &terms,
            U256::one(),
        )
        .expect("a correctly priced proposal is accepted");
    let one = approve_at(&mut w, 0, &draft, U256::one()).expect("0");
    let done = approve_at(&mut w, 1, &one, U256::one()).expect("1");
    let raw = done.finish(&CHAIN).expect("finish").raw_hex;
    assert_eq!(u64::try_from(raw.len() / 2).expect("len"), size);
}

#[test]
fn an_approval_for_another_account_than_the_open_one_is_refused() {
    // Wallet accounts 0 and 1 are members of two funded accounts. A spend
    // from the second is handed over while the first is open on the screen.
    let mut w = wallet();
    let open = account(&w);
    let other = MultisigAccount::new(
        1,
        &[w.member_key(0).expect("0"), w.member_key(1).expect("1")],
    )
    .expect("1-of-2");
    let theirs = w
        .propose_multisig(
            0,
            &other,
            RECIPIENT,
            U256::from(500),
            &terms(U256::zero()),
            U256::zero(),
        )
        .expect("propose");
    assert!(matches!(
        w.confirm_multisig(0, &open, &theirs, &CHAIN, &Context::default()),
        Err(WalletError::Multisig(_))
    ));
    // The refusal marked nothing reviewed, so nothing can be approved.
    let hash = maya_wallet_core::confirm::confirmation(theirs.transaction(), &CHAIN).signing_hash;
    assert!(matches!(
        w.approve_multisig(0, &theirs, &CHAIN, &hash, U256::zero()),
        Err(WalletError::Signing(_))
    ));
    // With its own account open, the same spend is shown and approved.
    let shown = w
        .confirm_multisig(0, &other, &theirs, &CHAIN, &Context::default())
        .expect("its own account");
    assert_eq!(shown.signing_hash, hash);
    assert!(
        w.approve_multisig(0, &theirs, &CHAIN, &hash, U256::zero())
            .is_ok()
    );
}

#[test]
fn the_confirm_text_names_the_policy_and_the_signer() {
    let mut w = wallet();
    let p = propose(&w);
    let shown = w
        .confirm_multisig(1, &account(&w), &p, &CHAIN, &Context::default())
        .expect("member 1");
    assert!(
        shown
            .description
            .starts_with("Multisig 2 of 3, signing as member 1 (account 1). "),
        "{}",
        shown.description
    );
    let dialog = maya_wallet_core::confirm::approval_message(&shown);
    assert!(
        dialog.contains("Multisig 2 of 3, signing as member 1 (account 1)"),
        "{dialog}"
    );
    // A non-member is refused before any confirm screen.
    assert!(matches!(
        w.confirm_multisig(5, &account(&w), &p, &CHAIN, &Context::default()),
        Err(WalletError::Multisig(_))
    ));
}

#[test]
fn a_hardened_member_index_is_refused_not_aliased() {
    // 2^31 would otherwise be ORed into account 0's hardened slot.
    let w = wallet();
    assert!(w.member_key(0x8000_0000).is_err());
    assert!(w.member_key(u32::MAX).is_err());
}

#[test]
fn a_proposal_for_an_account_this_wallet_refuses_to_open_is_refused_when_read() {
    // The same spendable() rule as MultisigAccount::new and from_hex: a
    // proposal naming a policy no hybrid quorum can spend is not approved.
    let w = wallet();
    let p = propose(&w);
    let bytes = hex::decode(ledger_bound_policy_hex(&w)).expect("hex");
    let (policy, _) = MultisigPolicy::decode(&bytes).expect("policy");
    let text = reshaped(&p, |tx| {
        tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy)));
    });
    assert!(refused(&text));
}
