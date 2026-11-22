//! The Multisig screen's command layer (P-16), driven as the screen drives it:
//! hex text in and out, one session per wallet, no node and no native dialog.
//! Each `#[tauri::command]` above is a thin wrapper over the functions called
//! here; what is not covered is the wrapper itself (the Tauri `State`, the
//! address-book read for the review context, the native dialog) and the
//! node calls inside `multisig_fee_options` (`fee_quote`, `network_status`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_clear_sign::Context;
use maya_wallet_core::multisig::{MemberKey, MultisigAccount};
use maya_wallet_core::payment::priced_fee_tiers;
use maya_wallet_core::wallet::Wallet;
use primitive_types::U256;

use super::{
    ApprovalRequest, Spend, approval_to_show, approve_shown, create, multisig_finish,
    multisig_merge, multisig_status, preview, propose,
};
use crate::commands::Session;

const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon about";
const GENESIS: &str = "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a";
const RECIPIENT: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn session() -> Session {
    let mut wallet = Wallet::from_mnemonic(PHRASE, "").expect("phrase");
    // Unlock derives the accounts; only derived ones mark a member as "you".
    wallet.account(0).expect("derive 0");
    wallet.account(1).expect("derive 1");
    let session = Session::default();
    session.open("primary", wallet);
    session
}

fn member(session: &Session, index: u32) -> MemberKey {
    session
        .with(|w| w.member_key(index).map_err(|e| e.to_string()))
        .expect("member")
}

/// The node's fee collector (`state::fees::FEE_COLLECTOR`); the core refuses
/// any other address, so a wrong value here fails the test.
fn collector() -> String {
    let mut address = [0u8; 32];
    address[..23].copy_from_slice(b"maya2c/fee/collector/v1");
    hex::encode(address)
}

fn request<'a>(
    index: u32,
    account: &'a str,
    proposal: &'a str,
    base_fee: &'a str,
) -> ApprovalRequest<'a> {
    ApprovalRequest {
        index,
        account_hex: account,
        proposal_hex: proposal,
        genesis: GENESIS,
        base_fee,
    }
}

/// Member `index` previews, then approves what the preview showed.
fn approve(
    session: &Session,
    index: u32,
    account: &str,
    proposal: &str,
    base_fee: &str,
) -> super::ProposalView {
    let ctx = Context::default();
    let shown = preview(index, account, proposal, GENESIS, &ctx, session).expect("preview");
    let req = request(index, account, proposal, base_fee);
    let checked = approval_to_show(&req, &shown.signing_hash, &ctx, session).expect("checked");
    approve_shown(&req, &checked, session).expect("approve")
}

#[test]
fn create_propose_price_approve_merge_and_finish_through_the_commands() {
    let session = session();
    let members = [member(&session, 0), member(&session, 1)];
    let card = create(2, &members, GENESIS, &session).expect("create");
    assert_eq!(card.threshold, 2);
    assert!(card.holds_member);

    // Fee pre-fill: the size multisig_fee_options prices, at 1 per byte.
    let size = MultisigAccount::from_hex(&card.account_hex)
        .and_then(|a| a.spend_size())
        .expect("size");
    let base_fee = "1";
    let [_, standard, _] = priced_fee_tiers(U256::one(), size).expect("tiers");
    let (fee, collector) = (standard.to_string(), collector());
    let spend = Spend {
        recipient: RECIPIENT,
        amount: "1000000",
        fee: &fee,
        fee_to: Some(&collector),
        nonce: 4,
    };
    let draft =
        propose(0, &card.account_hex, &spend, GENESIS, base_fee, &session).expect("propose");
    assert!(draft.approved_by.is_empty() && !draft.complete);

    // Two members approve the same draft in parallel, as on two machines.
    let a = approve(
        &session,
        0,
        &card.account_hex,
        &draft.proposal_hex,
        base_fee,
    );
    let b = approve(
        &session,
        1,
        &card.account_hex,
        &draft.proposal_hex,
        base_fee,
    );
    assert_eq!(
        (a.approved_by.clone(), b.approved_by.clone()),
        (vec![0], vec![1])
    );
    assert!(
        multisig_finish(a.proposal_hex.clone(), GENESIS.to_string()).is_err(),
        "1 of 2"
    );

    let merged =
        multisig_merge(a.proposal_hex, b.proposal_hex, GENESIS.to_string()).expect("merge");
    assert_eq!(merged.approved_by, [0, 1]);
    assert!(merged.complete);
    let status = multisig_status(merged.proposal_hex.clone(), GENESIS.to_string()).expect("status");
    assert_eq!(status.approved_by, merged.approved_by);
    // The draft text still reads as unapproved: status comes from the text.
    let still = multisig_status(draft.proposal_hex, GENESIS.to_string()).expect("draft");
    assert!(still.approved_by.is_empty());

    let signed = multisig_finish(merged.proposal_hex, GENESIS.to_string()).expect("finish");
    assert_eq!(signed.recipient, RECIPIENT);
    assert_eq!(signed.amount, U256::from(1_000_000));
    assert_eq!(signed.nonce, 4);
    assert!(!signed.raw_hex.is_empty());
}

#[test]
fn an_approval_is_refused_before_the_dialog_when_the_hash_or_the_fee_is_wrong() {
    let session = session();
    let members = [member(&session, 0), member(&session, 1)];
    let card = create(2, &members, GENESIS, &session).expect("create");
    let spend = Spend {
        recipient: RECIPIENT,
        amount: "500",
        fee: "0",
        fee_to: None,
        nonce: 4,
    };
    let draft = propose(0, &card.account_hex, &spend, GENESIS, "0", &session).expect("propose");
    let ctx = Context::default();

    // A hash other than the one the confirm screen showed.
    let req = request(0, &card.account_hex, &draft.proposal_hex, "0");
    let error = approval_to_show(&req, &"00".repeat(32), &ctx, &session).expect_err("hash");
    assert!(
        error.contains("differs from the one on the confirm screen"),
        "{error}"
    );

    // A fee-off draft approved on a chain charging 1 per byte would leave the
    // finished spend underpaid: refused before anyone is asked to approve.
    let shown = preview(
        0,
        &card.account_hex,
        &draft.proposal_hex,
        GENESIS,
        &ctx,
        &session,
    )
    .expect("preview");
    let priced = request(0, &card.account_hex, &draft.proposal_hex, "1");
    assert!(approval_to_show(&priced, &shown.signing_hash, &ctx, &session).is_err());
}

#[test]
fn a_locked_session_or_another_open_account_is_refused() {
    let locked = Session::default();
    let members = [member(&session(), 0), member(&session(), 1)];
    assert_eq!(
        create(2, &members, GENESIS, &locked).expect_err("locked"),
        "the wallet is locked"
    );

    let session = session();
    let card = create(2, &members, GENESIS, &session).expect("create");
    let spend = Spend {
        recipient: RECIPIENT,
        amount: "500",
        fee: "0",
        fee_to: None,
        nonce: 4,
    };
    let draft = propose(0, &card.account_hex, &spend, GENESIS, "0", &session).expect("propose");
    // The screen has a 1-of-2 over the same keys open: not the draft's account.
    let other = create(1, &members, GENESIS, &session).expect("other");
    let ctx = Context::default();
    assert!(
        preview(
            0,
            &other.account_hex,
            &draft.proposal_hex,
            GENESIS,
            &ctx,
            &session
        )
        .is_err()
    );
    // A malformed genesis is refused by name.
    let req = ApprovalRequest {
        genesis: "zz",
        ..request(0, &card.account_hex, &draft.proposal_hex, "0")
    };
    assert!(
        approval_to_show(&req, "", &ctx, &session)
            .expect_err("genesis")
            .contains("genesis")
    );
}
