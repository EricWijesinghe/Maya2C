//! Real-world assets end to end: issuance, DvP, gated transfers, and ten
//! thousand dividends in one block.
//!
//! ## The two tests that matter
//!
//! `a_dvp_that_cannot_settle_leaves_the_block_valid` is invariant 7 as a
//! property. A DvP that errored would let anyone void any block by submitting a
//! swap they know cannot settle, and the counterparty need not be involved.
//!
//! `ten_thousand_dividends_settle_in_one_block` is the brief's number, and what
//! it actually checks is conservation: the holders' balances rise by exactly
//! what the issuer's fell. A rounding rule that lost a base unit would be
//! refused by the invariant guard, so this is correctness rather than fairness.

use custom_l1_node::core::rwa_payload::{
    AttestLegal, DistributeRevenue, IssueRwa, RecordEligibility, RulePayload, SettleDvp,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};

use maya_rwa::cap_table::{CapTablePage, HOLDERS_PER_PAGE, Holder};

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct Fixture {
    db: StateDB,
    _dir: TempDir,
}

fn fixture(funded: &[(Address, u64)]) -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    for (address, balance) in funded {
        db.put_account(
            address,
            &Account {
                balance: *balance,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    Fixture { db, _dir: dir }
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

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key).expect("sign");
    tx
}

fn apply(db: &StateDB, kinds: Vec<(TxKind, u64)>, key: &HybridSigningKey, height: u64) {
    let block = block_of(
        kinds
            .into_iter()
            .map(|(kind, nonce)| signed(kind, nonce, key))
            .collect(),
    );
    db.apply_block(&block, BlockContext::at_height(height))
        .expect("apply");
}

fn try_apply(
    db: &StateDB,
    kind: TxKind,
    nonce: u64,
    key: &HybridSigningKey,
    height: u64,
) -> Result<(), NodeError> {
    db.apply_block(
        &block_of(vec![signed(kind, nonce, key)]),
        BlockContext::at_height(height),
    )
    .map(|_| ())
}

const LABEL: &str = "Building A";

fn issue(rule: Option<RulePayload>, units: u64) -> TxKind {
    TxKind::IssueRwa(Box::new(IssueRwa {
        label: LABEL.to_owned(),
        total_units: units,
        rule,
    }))
}

fn asset_of(issuer: &Address) -> [u8; 32] {
    custom_l1_node::state::rwa::derive_asset(issuer, LABEL)
}

fn balance(db: &StateDB, address: &Address) -> u64 {
    db.get_account(address).expect("account").balance
}

// ---------------------------------------------------------------------------
// 1. issuance
// ---------------------------------------------------------------------------

#[test]
fn issuance_seats_the_whole_supply_with_the_issuer() {
    // A token whose units exist but are held by nobody would make every
    // distribution's weights sum to less than the supply, and the difference
    // would have no owner.
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 1_000_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);

    let asset = asset_of(&issuer.address());
    let token = fixture
        .db
        .stored_rwa_token(&asset)
        .expect("read")
        .expect("issued");
    assert_eq!(token.total_units, 1_000);
    assert_eq!(token.issuer, issuer.address());

    let page = fixture.db.stored_cap_table_page(&asset, 0).expect("page");
    assert_eq!(page.units_of(&issuer.address()), 1_000);
    assert_eq!(page.total_units().expect("total"), token.total_units);
}

#[test]
fn issuing_the_same_label_twice_is_refused() {
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 1_000_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);
    assert!(try_apply(&fixture.db, issue(None, 500), 1, &issuer, 2).is_err());
}

// ---------------------------------------------------------------------------
// 2. delivery versus payment
// ---------------------------------------------------------------------------

fn dvp(asset: [u8; 32], seller: Address, units: u64, price: u64) -> TxKind {
    TxKind::SettleDvp(Box::new(SettleDvp {
        asset,
        seller,
        units,
        price,
        page: 0,
    }))
}

#[test]
fn a_dvp_moves_both_legs_or_neither() {
    let issuer = generate_signing_key().expect("keygen");
    let buyer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 1_000), (buyer.address(), 5_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);

    let asset = asset_of(&issuer.address());
    apply(
        &fixture.db,
        vec![(dvp(asset, issuer.address(), 100, 4_000), 0)],
        &buyer,
        2,
    );

    // Both legs.
    let page = fixture.db.stored_cap_table_page(&asset, 0).expect("page");
    assert_eq!(page.units_of(&buyer.address()), 100);
    assert_eq!(page.units_of(&issuer.address()), 900);
    assert_eq!(balance(&fixture.db, &buyer.address()), 1_000);
    assert_eq!(balance(&fixture.db, &issuer.address()), 5_000);
}

#[test]
fn a_dvp_that_cannot_settle_leaves_the_block_valid() {
    // Invariant 7 as a property. A failing transaction fails its whole block
    // here, so a DvP that errored would hand anyone a way to void any block —
    // and the counterparty need not be involved at all.
    let issuer = generate_signing_key().expect("keygen");
    let buyer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 1_000), (buyer.address(), 10)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);
    let asset = asset_of(&issuer.address());

    let before_buyer = balance(&fixture.db, &buyer.address());
    let before_issuer = balance(&fixture.db, &issuer.address());
    let before_page = fixture.db.stored_cap_table_page(&asset, 0).expect("page");

    for (nonce, (units, price, why)) in [
        (100u64, 999_999u64, "the buyer cannot pay"),
        (999_999, 5, "the seller does not hold the units"),
        (0, 5, "a swap of nothing"),
    ]
    .into_iter()
    .enumerate()
    {
        // The nonce still advances. That is the point: the transaction is
        // valid, it was paid for, and it moved nothing.
        try_apply(
            &fixture.db,
            dvp(asset, issuer.address(), units, price),
            nonce as u64,
            &buyer,
            2 + nonce as u64,
        )
        .unwrap_or_else(|error| panic!("{why} voided the block: {error}"));

        assert_eq!(
            balance(&fixture.db, &buyer.address()),
            before_buyer,
            "{why}"
        );
        assert_eq!(
            balance(&fixture.db, &issuer.address()),
            before_issuer,
            "{why}"
        );
        assert_eq!(
            fixture.db.stored_cap_table_page(&asset, 0).expect("page"),
            before_page,
            "{why}"
        );
    }

    // An unknown asset is the same: a no-op, not a weapon.
    try_apply(
        &fixture.db,
        dvp([0xaa; 32], issuer.address(), 1, 1),
        3,
        &buyer,
        9,
    )
    .expect("an unknown asset voided the block");
}

// ---------------------------------------------------------------------------
// 3. jurisdictional rules
// ---------------------------------------------------------------------------

fn gated_rule(trusted: Address) -> RulePayload {
    RulePayload {
        schema: [7; 32],
        // 2 is EqualTo — a jurisdiction code.
        predicate_tag: 2,
        bound: 826,
        eligibility_blocks: 100,
        trusted_issuers: vec![trusted],
    }
}

#[test]
fn a_gated_asset_refuses_a_buyer_with_no_eligibility() {
    // And refuses it as a no-op, like every other DvP that cannot settle.
    let issuer = generate_signing_key().expect("keygen");
    let buyer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 1_000), (buyer.address(), 10_000)]);
    apply(
        &fixture.db,
        vec![(issue(Some(gated_rule(issuer.address())), 1_000), 0)],
        &issuer,
        1,
    );
    let asset = asset_of(&issuer.address());

    try_apply(
        &fixture.db,
        dvp(asset, issuer.address(), 100, 4_000),
        0,
        &buyer,
        2,
    )
    .expect("an ineligible buyer voided the block");
    let page = fixture.db.stored_cap_table_page(&asset, 0).expect("page");
    assert_eq!(page.units_of(&buyer.address()), 0);
    assert_eq!(balance(&fixture.db, &buyer.address()), 10_000);
}

#[test]
fn an_eligible_buyer_settles_and_the_record_expires() {
    let issuer = generate_signing_key().expect("keygen");
    let buyer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 1_000), (buyer.address(), 10_000)]);
    apply(
        &fixture.db,
        vec![(issue(Some(gated_rule(issuer.address())), 1_000), 0)],
        &issuer,
        1,
    );
    let asset = asset_of(&issuer.address());

    apply(
        &fixture.db,
        vec![(TxKind::RecordEligibility(RecordEligibility { asset }), 0)],
        &buyer,
        10,
    );
    let eligibility = fixture
        .db
        .stored_eligibility(&asset, &buyer.address())
        .expect("read")
        .expect("recorded");
    assert_eq!(eligibility.verified_at, 10);
    assert_eq!(eligibility.expires_at, 110);
    assert!(eligibility.is_live(109));
    assert!(!eligibility.is_live(110));

    apply(
        &fixture.db,
        vec![(dvp(asset, issuer.address(), 50, 1_000), 1)],
        &buyer,
        20,
    );
    let page = fixture.db.stored_cap_table_page(&asset, 0).expect("page");
    assert_eq!(page.units_of(&buyer.address()), 50);

    // Past the window the same buyer is refused again — and refused as a no-op.
    try_apply(
        &fixture.db,
        dvp(asset, issuer.address(), 50, 1_000),
        2,
        &buyer,
        200,
    )
    .expect("a stale eligibility voided the block");
    let page = fixture.db.stored_cap_table_page(&asset, 0).expect("page");
    assert_eq!(
        page.units_of(&buyer.address()),
        50,
        "a stale record still settled"
    );
}

#[test]
fn an_eligibility_record_for_an_ungated_asset_is_refused() {
    // It would be a record nothing reads.
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 1_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);
    let asset = asset_of(&issuer.address());
    assert!(
        try_apply(
            &fixture.db,
            TxKind::RecordEligibility(RecordEligibility { asset }),
            1,
            &issuer,
            2
        )
        .is_err()
    );
}

// ---------------------------------------------------------------------------
// 4. ten thousand dividends
// ---------------------------------------------------------------------------

/// Seats `count` holders across as many pages as they need.
fn seat_holders(db: &StateDB, asset: &[u8; 32], count: usize) -> Vec<Address> {
    let mut addresses = Vec::with_capacity(count);
    let mut page_holders: Vec<Holder> = Vec::with_capacity(HOLDERS_PER_PAGE);
    let mut page_index = 0u32;

    for index in 0..count {
        let mut address = [0u8; 32];
        address[..8].copy_from_slice(&(index as u64 + 1).to_le_bytes());
        addresses.push(address);
        page_holders.push(Holder {
            address,
            // Uneven weights on purpose: equal ones would divide exactly and
            // never exercise the remainder rule.
            units: (index as u64 % 97) + 1,
        });

        if page_holders.len() == HOLDERS_PER_PAGE {
            let page = CapTablePage::new(*asset, page_index, std::mem::take(&mut page_holders))
                .expect("page");
            db.raw_put_for_test(
                &custom_l1_node::state::rwa::cap_table_key(asset, page_index),
                &page.encode(),
            )
            .expect("seed page");
            page_index += 1;
        }
    }
    if !page_holders.is_empty() {
        let page = CapTablePage::new(*asset, page_index, page_holders).expect("page");
        db.raw_put_for_test(
            &custom_l1_node::state::rwa::cap_table_key(asset, page_index),
            &page.encode(),
        )
        .expect("seed page");
    }
    addresses
}

#[test]
fn ten_thousand_dividends_settle_in_one_block() {
    // The brief's number, and what it actually checks is conservation: the
    // holders' balances rise by exactly what the issuer's fell. A rounding rule
    // that lost a base unit would be refused by the invariant guard, so this is
    // a correctness property rather than a fairness one.
    const HOLDERS: usize = 10_000;
    const REVENUE: u64 = 123_456_789;

    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), REVENUE + 1_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000_000), 0)], &issuer, 1);

    let asset = asset_of(&issuer.address());
    let holders = seat_holders(&fixture.db, &asset, HOLDERS);
    let pages = (HOLDERS.div_ceil(HOLDERS_PER_PAGE)) as u32;

    let before_issuer = balance(&fixture.db, &issuer.address());
    let before_holders: u64 = holders
        .iter()
        .map(|address| balance(&fixture.db, address))
        .sum();

    apply(
        &fixture.db,
        vec![(
            TxKind::DistributeRevenue(Box::new(DistributeRevenue {
                asset,
                round: 1,
                total: REVENUE,
                pages,
            })),
            1,
        )],
        &issuer,
        2,
    );

    let after_issuer = balance(&fixture.db, &issuer.address());
    let after_holders: u64 = holders
        .iter()
        .map(|address| balance(&fixture.db, address))
        .sum();

    assert_eq!(
        before_issuer - after_issuer,
        REVENUE,
        "the issuer paid the total"
    );
    assert_eq!(
        after_holders - before_holders,
        REVENUE,
        "the holders received exactly what the issuer paid"
    );

    let settled = fixture
        .db
        .stored_distribution(&asset, 1)
        .expect("read")
        .expect("settled");
    assert_eq!(settled.total, REVENUE);
    assert_eq!(settled.holders, HOLDERS as u32);
}

#[test]
fn a_round_cannot_be_settled_twice() {
    // Otherwise a replayed distribution pays every holder again out of an
    // issuer who only agreed to pay once.
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 100_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);
    let asset = asset_of(&issuer.address());

    let round = |nonce: u64| {
        (
            TxKind::DistributeRevenue(Box::new(DistributeRevenue {
                asset,
                round: 1,
                total: 1_000,
                pages: 1,
            })),
            nonce,
        )
    };
    apply(&fixture.db, vec![round(1)], &issuer, 2);
    assert!(try_apply(&fixture.db, round(2).0, 2, &issuer, 3).is_err());
}

#[test]
fn only_the_issuer_distributes_and_only_within_the_page_bound() {
    let issuer = generate_signing_key().expect("keygen");
    let stranger = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 100_000), (stranger.address(), 100_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);
    let asset = asset_of(&issuer.address());

    let distribution = |pages: u32| {
        TxKind::DistributeRevenue(Box::new(DistributeRevenue {
            asset,
            round: 9,
            total: 100,
            pages,
        }))
    };
    assert!(try_apply(&fixture.db, distribution(1), 0, &stranger, 2).is_err());
    // Past the page bound: a block whose validation cost nobody can predict.
    assert!(try_apply(&fixture.db, distribution(1_000), 1, &issuer, 3).is_err());
}

// ---------------------------------------------------------------------------
// 5. legal attestations carry a hash, never a document
// ---------------------------------------------------------------------------

#[test]
fn a_legal_attestation_records_a_hash_and_a_reference() {
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 10_000)]);
    apply(&fixture.db, vec![(issue(None, 1_000), 0)], &issuer, 1);
    let asset = asset_of(&issuer.address());

    apply(
        &fixture.db,
        vec![(
            TxKind::AttestLegal(Box::new(AttestLegal {
                asset,
                document: [0x5c; 32],
                reference: "ipfs://bafybeideed".to_owned(),
            })),
            1,
        )],
        &issuer,
        5,
    );

    let attestation = fixture
        .db
        .stored_legal_attestation(&asset, &[0x5c; 32])
        .expect("read")
        .expect("recorded");
    assert_eq!(attestation.document, [0x5c; 32]);
    assert_eq!(attestation.recorded_at, 5);
    // A hash and a reference and nothing else: a prospectus on a permanent
    // public chain cannot be corrected, and it likely names people.
    assert_eq!(
        attestation.encode().len(),
        32 + 32 + 32 + 8 + 1 + attestation.reference.len()
    );
}
