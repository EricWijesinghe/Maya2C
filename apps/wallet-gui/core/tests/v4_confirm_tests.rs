//! The confirm screen for the v4 forms (ADR-135 I7): tokens, unique assets,
//! names, scheduled payments and sponsored fees.
//!
//! Each form must reach the signer in the node's own words with every account
//! in full Bech32m, and the forms that hand over control or spend a balance
//! must raise the same review warnings a plain transfer would.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::asset_mint_payload::AssetMintAction;
use custom_l1_node::core::name_payload::{NameAction, NameUpdate};
use custom_l1_node::core::nft_payload::NftAction;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::schedule_payload::ScheduleAction;
use custom_l1_node::core::{ChainTag, Transaction};
use maya_amount::Amount;
use maya_clear_sign::{Context, Network, Warning, grouped};
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite, SuiteId, suite_address};
use maya_wallet_core::amount::format_display;
use maya_wallet_core::confirm::{
    GovernedFees, NAME_FEE_NOT_SHOWN, RUN_FEE_NOT_SHOWN, approval_message, confirmation,
    confirmation_with, confirmation_with_fees, warning_text,
};
use primitive_types::U256;

const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);
const A: [u8; 32] = [0x41; 32];
const B: [u8; 32] = [0x42; 32];

/// Every v4 form, with the accounts its confirm screen must name in full.
fn forms() -> Vec<(&'static str, TxKind, Vec<[u8; 32]>)> {
    vec![
        (
            "register capped",
            TxKind::AssetMint(AssetMintAction::RegisterCapped {
                symbol: *b"GOLD\0\0\0\0",
                total_supply: U256::from(1),
                cap: U256::from(2),
                authority: A,
            }),
            vec![A],
        ),
        (
            "mint",
            TxKind::AssetMint(AssetMintAction::Mint {
                asset: [0x10; 32],
                to: A,
                amount: U256::from(5),
            }),
            vec![A],
        ),
        (
            "burn",
            TxKind::AssetMint(AssetMintAction::Burn {
                asset: [0x10; 32],
                amount: U256::from(5),
            }),
            vec![],
        ),
        (
            "renounce",
            TxKind::AssetMint(AssetMintAction::Renounce { asset: [0x10; 32] }),
            vec![],
        ),
        (
            "create collection",
            TxKind::Nft(NftAction::CreateCollection {
                max_items: 3,
                metadata_hash: [0x11; 32],
            }),
            vec![],
        ),
        (
            "mint item",
            TxKind::Nft(NftAction::MintItem {
                collection: [0x12; 32],
                metadata_hash: [0x11; 32],
            }),
            vec![],
        ),
        (
            "transfer item",
            TxKind::Nft(NftAction::TransferItem {
                collection: [0x12; 32],
                index: 0,
                to: A,
            }),
            vec![A],
        ),
        (
            "burn item",
            TxKind::Nft(NftAction::BurnItem {
                collection: [0x12; 32],
                index: 0,
            }),
            vec![],
        ),
        (
            "commit name",
            TxKind::Name(NameAction::Commit {
                commitment: [0x13; 32],
            }),
            vec![],
        ),
        (
            "register name",
            TxKind::Name(NameAction::Register {
                label: b"alice".to_vec(),
                salt: [0x14; 32],
                periods: 1,
                target: A,
            }),
            vec![A],
        ),
        (
            "renew name",
            TxKind::Name(NameAction::Renew {
                label: b"alice".to_vec(),
                periods: 1,
            }),
            vec![],
        ),
        (
            "point name",
            TxKind::Name(NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::SetTarget(A),
            }),
            vec![A],
        ),
        (
            "give name",
            TxKind::Name(NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::Transfer(B),
            }),
            vec![B],
        ),
        (
            "create schedule",
            TxKind::Schedule(ScheduleAction::Create {
                payee: A,
                amount: U256::from(4),
                first_due: 10,
                interval: 20,
                runs: 3,
            }),
            vec![A],
        ),
        (
            "cancel schedule",
            TxKind::Schedule(ScheduleAction::Cancel {
                schedule_id: [0x15; 32],
            }),
            vec![],
        ),
    ]
}

#[test]
fn every_v4_form_names_its_accounts_in_full_bech32m_and_never_in_hex() {
    for (name, kind, accounts) in forms() {
        let shown = confirmation(&Transaction::with_kind(kind, 1), &CHAIN);
        let text = approval_message(&shown);
        let screen = format!("{text}\n{:?}", shown.details);
        assert!(
            shown.description.ends_with('.'),
            "{name}: {}",
            shown.description
        );
        for account in &accounts {
            let full = grouped(Network::Devnet, account);
            assert!(screen.contains(&full), "{name}: {full} missing\n{screen}");
            assert!(
                !screen.contains(&hex::encode(account)),
                "{name}: hex\n{screen}"
            );
            let short = format!("{:02x}{:02x}\u{2026}", account[0], account[1]);
            assert!(!text.contains(&short), "{name}: abbreviated\n{text}");
        }
        assert!(
            shown.sponsor.is_none(),
            "{name}: no sponsor on a plain frame"
        );
    }
}

fn warnings(kind: TxKind, ctx: &Context) -> Vec<String> {
    confirmation_with(&Transaction::with_kind(kind, 1), &CHAIN, ctx).warnings
}

fn text(w: Warning) -> String {
    warning_text(w).to_string()
}

fn warnings_with(kind: TxKind, ctx: &Context, fees: &GovernedFees) -> Vec<String> {
    confirmation_with_fees(&Transaction::with_kind(kind, 1), &CHAIN, ctx, fees).warnings
}

#[test]
fn handing_over_or_re_pointing_a_name_or_a_mint_raises_the_control_change_warning() {
    let ctx = Context::default();
    let give = forms().remove(12).1;
    assert_eq!(warnings(give, &ctx), vec![text(Warning::ControlChange)]);
    let renounce = forms().remove(3).1;
    assert_eq!(warnings(renounce, &ctx), vec![text(Warning::ControlChange)]);
    // Re-pointing redirects every later payment made to the name.
    let point = forms().remove(11).1;
    assert_eq!(warnings(point, &ctx), vec![text(Warning::ControlChange)]);
    // Committing hands nothing over and costs nothing.
    assert!(warnings(forms().remove(8).1, &ctx).is_empty());
}

#[test]
fn a_name_fee_is_reviewed_as_a_burn_or_said_to_be_unshown() {
    // Not read from the node: the screen says a fee applies.
    for i in [9, 10] {
        assert_eq!(
            warnings(forms().remove(i).1, &Context::default()),
            vec![NAME_FEE_NOT_SHOWN.to_string()],
            "form {i}"
        );
    }
    // Read: 10 per period x 1 period burns all of a balance of 10.
    let fees = GovernedFees {
        name_fee_per_period: Some(U256::from(10)),
        ..GovernedFees::default()
    };
    let ctx = Context {
        balances: vec![([0; 32], Amount::from_u64(10))],
        ..Context::default()
    };
    assert_eq!(
        warnings_with(forms().remove(9).1, &ctx, &fees),
        vec![text(Warning::DrainsBalance)]
    );
    let roomy = Context {
        balances: vec![([0; 32], Amount::from_u64(1_000))],
        ..Context::default()
    };
    assert!(warnings_with(forms().remove(10).1, &roomy, &fees).is_empty());
}

#[test]
fn a_schedule_is_reviewed_as_its_whole_escrow_leaving_the_balance_now() {
    let zero_fee = GovernedFees {
        schedule_run_fee: Some(U256::zero()),
        ..GovernedFees::default()
    };
    let balance = |n| Context {
        balances: vec![([0; 32], Amount::from_u64(n))],
        ..Context::default()
    };
    // 4 x 3 runs = 12 of a balance of 13: over 90% leaves when it is created.
    let schedule = || forms().remove(13).1;
    assert_eq!(
        warnings_with(schedule(), &balance(13), &zero_fee),
        vec![text(Warning::DrainsBalance)]
    );
    // A run fee of 2 makes the escrow (4 + 2) x 3 = 18: 18 of 20 is 90%. The
    // fee-free amount, 12 of 20, would have passed.
    let fee = GovernedFees {
        schedule_run_fee: Some(U256::from(2)),
        ..GovernedFees::default()
    };
    assert_eq!(
        warnings_with(schedule(), &balance(20), &fee),
        vec![text(Warning::DrainsBalance)]
    );
    assert!(warnings_with(schedule(), &balance(1_000), &fee).is_empty());
    // Not read from the node: the screen says the run fees are not included.
    assert_eq!(
        warnings(schedule(), &balance(1_000)),
        vec![RUN_FEE_NOT_SHOWN.to_string()]
    );
}

#[test]
fn destroying_an_item_is_reviewed_as_a_spend() {
    let ctx = Context {
        balances: vec![([0x12; 32], Amount::from_u64(1))],
        ..Context::default()
    };
    assert_eq!(
        warnings(forms().remove(7).1, &ctx),
        vec![text(Warning::DrainsBalance)]
    );
}

#[test]
fn creating_units_or_items_raises_no_spend_warning_by_decision() {
    // Registering, minting, opening a collection and minting an item spend
    // none of the sender's balance (DECISIONS 2026-11-14). Pinned, so the
    // choice is visible and changes only on purpose.
    let ctx = Context {
        balances: vec![
            ([0x10; 32], Amount::from_u64(5)),
            ([0; 32], Amount::from_u64(1)),
        ],
        ..Context::default()
    };
    for i in [0, 1, 4, 5] {
        assert!(warnings(forms().remove(i).1, &ctx).is_empty(), "form {i}");
    }
}

#[test]
fn an_item_sent_to_a_look_alike_of_a_contact_is_flagged() {
    let mut look_alike = A;
    look_alike[10] ^= 0x5A;
    let ctx = Context {
        contacts: [A].into_iter().collect(),
        ..Context::default()
    };
    let to_look_alike = TxKind::Nft(NftAction::TransferItem {
        collection: [0x12; 32],
        index: 0,
        to: look_alike,
    });
    assert!(warnings(to_look_alike, &ctx).contains(&text(Warning::LookAlikeRecipient)));
    assert!(
        warnings(forms().remove(6).1, &ctx).is_empty(),
        "the contact itself"
    );
}

#[test]
fn burning_nearly_all_of_an_asset_is_flagged_as_a_drain() {
    let ctx = Context {
        balances: vec![([0x10; 32], Amount::from_u64(5))],
        ..Context::default()
    };
    assert_eq!(
        warnings(forms().remove(2).1, &ctx),
        vec![text(Warning::DrainsBalance)]
    );
}

#[test]
fn a_sponsored_frame_says_who_pays_the_fee_in_the_native_dialog() {
    let sender = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([1; 32]));
    let sponsor = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([2; 32]));
    let payer = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&sponsor));
    let mut tx = Transaction::with_kind(forms().remove(1).1, 0);
    tx.sign_with_suite::<MlDsa87>(&sender, &CHAIN).unwrap();
    let unsponsored = confirmation(&tx, &CHAIN);
    tx.sponsor_with::<MlDsa87>(&sponsor, U256::from(9_000), 77, &CHAIN)
        .unwrap();

    let shown = confirmation(&tx, &CHAIN);
    let full = grouped(Network::Devnet, &payer);
    assert_eq!(shown.sponsor.as_deref(), Some(full.as_str()));
    assert_eq!(
        shown.sponsor_max_fee,
        Some(format_display(U256::from(9_000)))
    );
    assert!(shown.sponsor_verified);
    let dialog = approval_message(&shown);
    assert!(
        dialog.contains(&format!(
            "Sponsor: {full} offers to pay the fee, at most {} MAYA. Your signature does not depend on it.",
            format_display(U256::from(9_000))
        )),
        "{dialog}"
    );
    assert!(shown.description.contains(&full), "{}", shown.description);
    assert!(!dialog.contains(&hex::encode(payer)), "{dialog}");
    // The sponsor does not change what the sender signed.
    assert_eq!(shown.signing_hash, unsponsored.signing_hash);

    // A sponsorship whose signature does not hold (here: a raised ceiling the
    // sponsor never signed) is shown as a claim, not as a sponsor.
    let mut forged = tx.clone();
    if let Some(s) = forged.sponsor.as_mut() {
        s.max_fee = U256::from(9_001);
    }
    let claimed = confirmation(&forged, &CHAIN);
    assert!(!claimed.sponsor_verified);
    let dialog = approval_message(&claimed);
    assert!(
        dialog.contains(&format!(
            "Claimed sponsor (unverified): {full} offers to pay the fee"
        )),
        "{dialog}"
    );
    // The same sponsorship on another chain does not hold either.
    let elsewhere = ChainTag::from_genesis([0x5B; 32]);
    assert!(!confirmation(&tx, &elsewhere).sponsor_verified);
}
