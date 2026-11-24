//! The trading subsystem against real block execution.
//!
//! The engine's own arithmetic is tested in `crates/dex/tests/`. What is tested here
//! is everything the engine deliberately does not know about: escrow, refunds,
//! the batch running at the end of the block rather than in the middle of it,
//! and what happens to a pool when the chain reorganises.
//!
//! Two of these are load-bearing rather than merely nice:
//!
//! - **A missed slippage bound must not fail the block.** A failing transaction
//!   invalidates its whole block here, so if a swap that lost a race returned
//!   an error, any trader could void a block by submitting a swap that was
//!   going to lose.
//! - **A reorg must restore pool reserves.** A pool left holding the abandoned
//!   chain's reserves is not a detectable corruption. It is two plausible
//!   numbers that go on quoting a price.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::dex_payload::{
    AssetRegistration, AssetTransfer, LiquidityDeposit, LiquidityWithdrawal, OrderPlacement,
    PoolCreation, RouteLeg, SwapRequest, SwapRoute,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::state::{
    Account, Address, AssetId, BlockContext, Direction, NATIVE_ASSET, PRICE_SCALE, PoolRecord,
    Side, StateDB, derive_asset_id, derive_lp_asset, derive_pair_id,
};
use maya_governance::params::ParameterKey;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

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
    common::bind(&db);
    for (address, balance) in funded {
        db.put_account(
            address,
            &Account {
                balance: primitive_types::U256::from(*balance),
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
            version: 0,
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

fn symbol(text: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..text.len()].copy_from_slice(text.as_bytes());
    out
}

/// A price of `whole` quote units per whole base unit.
fn price(whole: u64) -> maya_ledger_math::Amount {
    maya_ledger_math::Amount::from_u64(whole)
        .checked_mul(PRICE_SCALE)
        .expect("fits")
}

/// The world every trading test starts from: one account holding native coin
/// and a registered asset, and a funded pool trading the two.
struct Market {
    fixture: Fixture,
    maker: HybridSigningKey,
    maker_address: Address,
    asset: AssetId,
    pair: [u8; 32],
    /// Next unused nonce for `maker`.
    nonce: u64,
}

const LP_FEE_BPS: u32 = 30;
const NATIVE_SEED: u64 = 1_000_000;
const ASSET_SUPPLY: u64 = 4_000_000;
const POOL_NATIVE: u64 = 100_000;
const POOL_ASSET: u64 = 400_000;

fn market() -> Market {
    market_with(generate_signing_key().expect("key"))
}

/// The same market, built from a key the caller chose.
///
/// Needed wherever two runs are compared: a fresh key means a fresh address,
/// and two state roots that differ only because the accounts are in different
/// places would look exactly like the failure these tests are hunting for.
fn market_with(maker: HybridSigningKey) -> Market {
    market_with_fee(maker, LP_FEE_BPS)
}

/// The same market, with the pool's own LP rate chosen by the caller.
fn market_with_fee(maker: HybridSigningKey, lp_fee_bps: u32) -> Market {
    let maker_address = maker.address();
    let fixture = fixture(&[(maker_address, NATIVE_SEED)]);

    let asset = derive_asset_id(&maker_address, 0, &symbol("USD"));
    // The native coin is all zeros, so it is always the canonical base.
    let pair = derive_pair_id(&NATIVE_ASSET, &asset, lp_fee_bps);

    let block = block_of(vec![
        signed(
            TxKind::RegisterAsset(AssetRegistration {
                symbol: symbol("USD"),
                total_supply: primitive_types::U256::from(ASSET_SUPPLY),
            }),
            0,
            &maker,
        ),
        signed(
            TxKind::CreatePool(PoolCreation {
                asset_a: NATIVE_ASSET,
                asset_b: asset,
                lp_fee_bps,
                amount_a: primitive_types::U256::from(POOL_NATIVE),
                amount_b: primitive_types::U256::from(POOL_ASSET),
            }),
            1,
            &maker,
        ),
    ]);
    fixture
        .db
        .apply_block(&block, BlockContext::at_height(1))
        .expect("set up market");

    Market {
        fixture,
        maker,
        maker_address,
        asset,
        pair,
        nonce: 2,
    }
}

impl Market {
    fn db(&self) -> &StateDB {
        &self.fixture.db
    }

    fn native(&self, address: &Address) -> u64 {
        self.db()
            .asset_balance(&NATIVE_ASSET, address)
            .map(|v| u64::try_from(v).expect("fits u64"))
            .expect("balance")
    }

    fn held(&self, address: &Address) -> u64 {
        self.db()
            .asset_balance(&self.asset, address)
            .map(|v| u64::try_from(v).expect("fits u64"))
            .expect("balance")
    }

    fn reserves(&self) -> (u64, u64) {
        let record = self.db().get_pool(&self.pair).expect("read").expect("pool");
        // Test pools are far below 2^64.
        (
            record.pool.reserve_base.to_u64().expect("small"),
            record.pool.reserve_quote.to_u64().expect("small"),
        )
    }

    /// Applies one block of transactions from `maker`, advancing its nonce.
    fn apply(&mut self, kinds: Vec<TxKind>, height: u64) -> Result<(), NodeError> {
        let transactions: Vec<Transaction> = kinds
            .into_iter()
            .enumerate()
            .map(|(offset, kind)| signed(kind, self.nonce + offset as u64, &self.maker))
            .collect();
        let count = transactions.len() as u64;
        let result = self
            .db()
            .apply_block(&block_of(transactions), BlockContext::at_height(height))
            .map(|_| ());
        if result.is_ok() {
            self.nonce += count;
        }
        result
    }
}

// ---------------------------------------------------------------------------
// assets
// ---------------------------------------------------------------------------

#[test]
fn registering_an_asset_credits_its_whole_supply_to_the_creator() {
    let market = market();
    // The pool took its share of the deposit; the rest is still held.
    assert_eq!(
        market.held(&market.maker_address),
        ASSET_SUPPLY - POOL_ASSET
    );
    assert_eq!(
        market.native(&market.maker_address),
        NATIVE_SEED - POOL_NATIVE
    );

    let record = market
        .db()
        .get_asset(&market.asset)
        .expect("read")
        .expect("registered");
    assert_eq!(
        record.total_supply,
        primitive_types::U256::from(ASSET_SUPPLY)
    );
    assert_eq!(record.creator, market.maker_address);
}

#[test]
fn an_asset_symbol_that_could_be_used_to_impersonate_is_refused() {
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);

    // Lowercase, punctuation, and interior padding are all out. The last is the
    // subtle one: two encodings of one ticker is a look-alike attack that needs
    // no unusual characters at all.
    for bad in [
        symbol("usd"),
        symbol("US.D"),
        [b'U', 0, b'S', 0, 0, 0, 0, 0],
    ] {
        let block = block_of(vec![signed(
            TxKind::RegisterAsset(AssetRegistration {
                symbol: bad,
                total_supply: primitive_types::U256::from(1_000),
            }),
            0,
            &key,
        )]);
        assert!(matches!(
            harness
                .db
                .apply_block(&block, BlockContext::at_height(1))
                .unwrap_err(),
            NodeError::InvalidAssetSymbol { .. }
        ));
    }
}

#[test]
fn an_asset_supply_past_the_price_cap_is_refused() {
    // #279 review F1: a 2^200 supply against one base unit overflowed the
    // pool price and froze the pair. The cap itself still registers.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);
    let cap = primitive_types::U256::from_little_endian(&maya_dex::MAX_ASSET_SUPPLY.to_le_bytes());
    let register = |supply, nonce| {
        block_of(vec![signed(
            TxKind::RegisterAsset(AssetRegistration {
                symbol: symbol("BIG"),
                total_supply: supply,
            }),
            nonce,
            &key,
        )])
    };
    assert!(matches!(
        harness
            .db
            .apply_block(&register(cap + 1, 0), BlockContext::at_height(1))
            .unwrap_err(),
        NodeError::InvalidAssetSymbol { .. }
    ));
    harness
        .db
        .apply_block(&register(cap, 0), BlockContext::at_height(1))
        .expect("the cap itself registers");
}

#[test]
fn the_native_coin_cannot_be_moved_through_the_asset_transfer_path() {
    // Two spending paths over one balance is one more than can be reasoned
    // about, and the second one would not go through the nonce check that the
    // first does.
    let mut market = market();
    let recipient = generate_signing_key().expect("key").address();

    let error = market
        .apply(
            vec![TxKind::TransferAsset(AssetTransfer {
                asset: NATIVE_ASSET,
                recipient,
                amount: primitive_types::U256::from(1),
            })],
            2,
        )
        .unwrap_err();

    assert!(matches!(error, NodeError::NativeAssetTransfer));
}

#[test]
fn an_asset_nobody_registered_cannot_be_transferred() {
    let mut market = market();
    let recipient = generate_signing_key().expect("key").address();

    let error = market
        .apply(
            vec![TxKind::TransferAsset(AssetTransfer {
                asset: [7u8; 32],
                recipient,
                amount: primitive_types::U256::from(1),
            })],
            2,
        )
        .unwrap_err();

    assert!(matches!(error, NodeError::UnknownAsset(_)));
}

#[test]
fn asset_supply_is_conserved_by_a_transfer() {
    let mut market = market();
    let recipient = generate_signing_key().expect("key").address();
    let before = market.held(&market.maker_address);

    market
        .apply(
            vec![TxKind::TransferAsset(AssetTransfer {
                asset: market.asset,
                recipient,
                amount: primitive_types::U256::from(12_345),
            })],
            2,
        )
        .expect("transfer");

    assert_eq!(market.held(&market.maker_address), before - 12_345);
    assert_eq!(market.held(&recipient), 12_345);
}

// ---------------------------------------------------------------------------
// liquidity
// ---------------------------------------------------------------------------

#[test]
fn creating_a_pool_seeds_it_and_mints_share_tokens() {
    let market = market();
    assert_eq!(market.reserves(), (POOL_NATIVE, POOL_ASSET));

    let shares = market
        .db()
        .asset_balance(&derive_lp_asset(&market.pair), &market.maker_address)
        .expect("shares");
    assert!(shares > primitive_types::U256::zero());

    // Shares are an asset like any other, so they have an issuing pool and can
    // be moved by the ordinary transfer path.
    let record = market
        .db()
        .get_pool(&market.pair)
        .expect("read")
        .expect("pool");
    assert_eq!(
        record.pool.total_shares,
        maya_ledger_math::Amount::from_u64(u64::try_from(shares).expect("fits u64") + 1_000),
        "locked minimum"
    );
}

/// Two big assets from one key, returned as (base, quote) in the pair's order.
fn two_big_assets(harness: &Fixture, key: &HybridSigningKey) -> (AssetId, AssetId) {
    let supply = U256::exp10(45);
    let register = |name: &str, nonce| {
        signed(
            TxKind::RegisterAsset(AssetRegistration {
                symbol: symbol(name),
                total_supply: supply,
            }),
            nonce,
            key,
        )
    };
    harness
        .db
        .apply_block(
            &block_of(vec![register("AAA", 0), register("BBB", 1)]),
            BlockContext::at_height(1),
        )
        .expect("register");
    let a = derive_asset_id(&key.address(), 0, &symbol("AAA"));
    let b = derive_asset_id(&key.address(), 1, &symbol("BBB"));
    if a < b { (a, b) } else { (b, a) }
}

fn create(base: AssetId, quote: AssetId, base_amount: U256, quote_amount: U256) -> TxKind {
    TxKind::CreatePool(PoolCreation {
        asset_a: base,
        asset_b: quote,
        lp_fee_bps: LP_FEE_BPS,
        amount_a: base_amount,
        amount_b: quote_amount,
    })
}

#[test]
fn a_pool_with_no_usable_price_cannot_be_created() {
    // Replay of the #279 review's frozen pair: base outweighs quote by more
    // than PRICE_SCALE, so spot_price is 0 and every batch on the pair would
    // refund for good, under a pair id nobody else can take.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);
    let (base, quote) = two_big_assets(&harness, &key);
    let lopsided = create(base, quote, U256::exp10(26), U256::one());
    let error = harness
        .db
        .apply_block(
            &block_of(vec![signed(lopsided, 2, &key)]),
            BlockContext::at_height(2),
        )
        .unwrap_err();
    assert!(matches!(error, NodeError::Trade { .. }), "{error}");
    let pair = derive_pair_id(&base, &quote, LP_FEE_BPS);
    assert!(harness.db.get_pool(&pair).expect("read").is_none());
    assert_eq!(
        harness
            .db
            .asset_balance(&base, &key.address())
            .expect("balance"),
        U256::exp10(45),
        "the refused creation debited the creator"
    );
    // The same ratio one unit inside the edge is a working pool.
    let edge = create(base, quote, U256::exp10(24), U256::one());
    harness
        .db
        .apply_block(
            &block_of(vec![signed(edge, 2, &key)]),
            BlockContext::at_height(2),
        )
        .expect("price exactly 1 is usable");
}

#[test]
fn a_batch_that_would_leave_no_usable_price_refunds_and_keeps_the_pool() {
    // A balanced pool, then one swap so large that the post-batch ratio passes
    // PRICE_SCALE. The batch is refused like an unpriceable one: the trader is
    // refunded, the reserves stay, and the block still applies.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);
    let (base, quote) = two_big_assets(&harness, &key);
    let seed = U256::exp10(13);
    harness
        .db
        .apply_block(
            &block_of(vec![signed(create(base, quote, seed, seed), 2, &key)]),
            BlockContext::at_height(2),
        )
        .expect("balanced pool");
    let pair = derive_pair_id(&base, &quote, LP_FEE_BPS);
    let before = harness.db.get_pool(&pair).expect("read").expect("pool");
    let held = harness
        .db
        .asset_balance(&base, &key.address())
        .expect("balance");

    let flood = TxKind::Swap(SwapRequest {
        pair,
        direction: Direction::BaseToQuote.tag(),
        amount_in: U256::exp10(40),
        min_out: U256::one(),
        deadline: 0,
    });
    harness
        .db
        .apply_block(
            &block_of(vec![signed(flood, 3, &key)]),
            BlockContext::at_height(3),
        )
        .expect("a refused batch never fails the block");
    assert_reserves_and_balance_kept(&harness, &pair, &before, &base, &key, held);
}

fn assert_reserves_and_balance_kept(
    harness: &Fixture,
    pair: &[u8; 32],
    before: &PoolRecord,
    base: &AssetId,
    key: &HybridSigningKey,
    held: U256,
) {
    let after = harness.db.get_pool(pair).expect("read").expect("pool");
    assert_eq!(after.pool.reserve_base, before.pool.reserve_base);
    assert_eq!(after.pool.reserve_quote, before.pool.reserve_quote);
    assert_eq!(
        harness
            .db
            .asset_balance(base, &key.address())
            .expect("balance"),
        held
    );
}

#[test]
fn a_route_that_would_leave_no_usable_price_does_not_execute() {
    // Replay: on master this route wrote (≈1e26, 1), spot price 0, and froze
    // the pair. Now it is a route that does not pay: no write, block valid.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);
    let (base, quote) = two_big_assets(&harness, &key);
    let pool = create(base, quote, U256::exp10(24) * 2, U256::from(2));
    harness
        .db
        .apply_block(
            &block_of(vec![signed(pool, 2, &key)]),
            BlockContext::at_height(2),
        )
        .expect("price 1 is usable");
    let pair = derive_pair_id(&base, &quote, LP_FEE_BPS);
    let before = harness.db.get_pool(&pair).expect("read").expect("pool");
    let held = harness
        .db
        .asset_balance(&base, &key.address())
        .expect("balance");

    let route = TxKind::SwapRoute(SwapRoute {
        legs: vec![RouteLeg {
            pair,
            direction: Direction::BaseToQuote.tag(),
        }],
        amount_in: U256::exp10(26),
        min_out: U256::one(),
        deadline: 0,
    });
    harness
        .db
        .apply_block(
            &block_of(vec![signed(route, 3, &key)]),
            BlockContext::at_height(3),
        )
        .expect("a route that does not pay never fails the block");
    assert_reserves_and_balance_kept(&harness, &pair, &before, &base, &key, held);
}

#[test]
fn an_honest_swap_still_clears_beside_a_flood_that_would_leave_no_price() {
    // Replay (#289 security review F1): one flood intent with min_out 1 used to
    // refund every honest swap in its batch. Now only the flood is dropped.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);
    let (base, quote) = two_big_assets(&harness, &key);
    let seed = U256::exp10(13);
    harness
        .db
        .apply_block(
            &block_of(vec![signed(create(base, quote, seed, seed), 2, &key)]),
            BlockContext::at_height(2),
        )
        .expect("balanced pool");
    let pair = derive_pair_id(&base, &quote, LP_FEE_BPS);
    let held_base = harness.db.asset_balance(&base, &key.address()).expect("b");
    let held_quote = harness.db.asset_balance(&quote, &key.address()).expect("q");
    let swap = |amount_in: U256, nonce| {
        let request = TxKind::Swap(SwapRequest {
            pair,
            direction: Direction::BaseToQuote.tag(),
            amount_in,
            min_out: U256::one(),
            deadline: 0,
        });
        signed(request, nonce, &key)
    };
    let honest = U256::exp10(10);
    harness
        .db
        .apply_block(
            &block_of(vec![swap(honest, 3), swap(U256::exp10(40), 4)]),
            BlockContext::at_height(3),
        )
        .expect("block");
    assert_eq!(
        harness.db.asset_balance(&base, &key.address()).expect("b"),
        held_base - honest,
        "only the honest swap is filled; the flood is refunded"
    );
    assert!(harness.db.asset_balance(&quote, &key.address()).expect("q") > held_quote);
}

/// What each side holds after one block of a split flood.
struct FloodResult {
    honest_in: U256,
    attacker_in: U256,
    honest: (U256, U256),
    attacker: (U256, U256),
}

/// One honest seller of H base and 255 attacker chunks of H - 1 base, all
/// base->quote on one pair in one block, against a 10^13 / 10^13 pool. The
/// honest swap asks for `honest_min_out`; the attacker's ask for one unit.
fn split_flood(honest_min_out: U256) -> FloodResult {
    // The 255 chunks fill the pair beside the honest swap: 256 intents is the
    // whole batch. One sender may hold only a sixteenth of it (ADR-154), so the
    // flood is spread over sixteen attacker accounts, fifteen of them full.
    let maker = generate_signing_key().expect("key");
    let honest = generate_signing_key().expect("key");
    let attackers: Vec<HybridSigningKey> = (0..16)
        .map(|_| generate_signing_key().expect("key"))
        .collect();
    let mut funded = vec![(maker.address(), 1_000), (honest.address(), 1_000)];
    funded.extend(attackers.iter().map(|a| (a.address(), 1_000)));
    let harness = fixture(&funded);
    let (base, quote) = two_big_assets(&harness, &maker);
    let seed = U256::exp10(13);
    harness
        .db
        .apply_block(
            &block_of(vec![signed(create(base, quote, seed, seed), 2, &maker)]),
            BlockContext::at_height(2),
        )
        .expect("balanced pool");
    let pair = derive_pair_id(&base, &quote, LP_FEE_BPS);

    let honest_amount = U256::exp10(24);
    let chunk = honest_amount - U256::one();
    let fund = |recipient: Address, amount: U256, nonce| {
        signed(
            TxKind::TransferAsset(AssetTransfer {
                asset: base,
                recipient,
                amount,
            }),
            nonce,
            &maker,
        )
    };
    // Chunks per attacker: sixteen each, the last one fifteen.
    let chunks = |index: usize| if index == 15 { 15_u64 } else { 16 };
    let mut funding = vec![fund(honest.address(), honest_amount, 3)];
    funding.extend(attackers.iter().enumerate().map(|(index, a)| {
        fund(
            a.address(),
            chunk * U256::from(chunks(index)),
            4 + index as u64,
        )
    }));
    harness
        .db
        .apply_block(&block_of(funding), BlockContext::at_height(3))
        .expect("funding");
    assert_eq!(
        harness
            .db
            .asset_balance(&base, &honest.address())
            .expect("balance"),
        honest_amount,
        "the honest seller is funded before the flood"
    );

    let swap = |sender: &HybridSigningKey, amount_in: U256, min_out: U256, nonce: u64| {
        signed(
            TxKind::Swap(SwapRequest {
                pair,
                direction: Direction::BaseToQuote.tag(),
                amount_in,
                min_out,
                deadline: 0,
            }),
            nonce,
            sender,
        )
    };
    let mut txs = vec![swap(&honest, honest_amount, honest_min_out, 0)];
    for (index, attacker) in attackers.iter().enumerate() {
        txs.extend((0..chunks(index)).map(|nonce| swap(attacker, chunk, U256::one(), nonce)));
    }
    harness
        .db
        .apply_block(&block_of(txs), BlockContext::at_height(4))
        .expect("a flood the batch refuses never fails the block");

    let held = |asset: &_, who: &HybridSigningKey| {
        harness
            .db
            .asset_balance(asset, &who.address())
            .expect("balance")
    };
    let flood = |asset: &_| {
        attackers
            .iter()
            .fold(U256::zero(), |sum, a| sum + held(asset, a))
    };
    FloodResult {
        honest_in: honest_amount,
        attacker_in: chunk * U256::from(255_u64),
        honest: (held(&base, &honest), held(&quote, &honest)),
        attacker: (flood(&base), flood(&quote)),
    }
}

#[test]
fn a_split_flood_fills_an_honest_seller_at_the_same_batch_price() {
    // B38, measured in CI (replacing the refund #308 first assumed): the flood
    // leaves spot at 1, not 0, so the largest-seller drop never fires. Every
    // intent clears at one uniform price, the honest seller included. With
    // min_out = 1 that crushed price is within the seller's own bound.
    let r = split_flood(U256::one());
    assert!(
        r.honest.0.is_zero() && r.attacker.0.is_zero(),
        "all base sold"
    );
    assert!(!r.honest.1.is_zero(), "the honest seller is paid");
    // Same price per unit: honest_out / honest_in == attacker_out / attacker_in,
    // up to one unit of floor rounding per intent (256 intents). The rounding
    // favours the pool: CI saw the honest fill at 39,062,499,999 and each
    // attacker chunk at 39,062,499,998, with the dust left in the reserve.
    let lhs = r.honest.1 * r.attacker_in;
    let rhs = r.attacker.1 * r.honest_in;
    let gap = if lhs > rhs { lhs - rhs } else { rhs - lhs };
    let slack = r.attacker_in + r.honest_in * U256::from(255_u64);
    assert!(
        gap <= slack,
        "honest {:?} vs attacker {:?}: not one batch price",
        r.honest,
        r.attacker
    );
}

#[test]
fn an_honest_seller_with_a_real_bound_is_refunded_not_crushed() {
    // Alone, 10^24 base into this pool pays about 10^13 quote. Asking for a
    // tenth of that turns the uniform flood price into a slippage violation:
    // the honest swap is dropped and its input comes back whole.
    let r = split_flood(U256::exp10(12));
    assert_eq!(
        r.honest,
        (r.honest_in, U256::zero()),
        "refunded in full (attacker {:?})",
        r.attacker
    );
}

#[test]
fn an_lp_whose_quote_share_rounds_to_zero_can_still_withdraw_its_base() {
    // Replay (#289 security review F3): with quote reserve 1, every partial
    // redemption rounds quote to 0, and ZeroOutput used to strand the base.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);
    let (base, quote) = two_big_assets(&harness, &key);
    let seed_base = U256::from(2_000_000_u64);
    harness
        .db
        .apply_block(
            &block_of(vec![signed(
                create(base, quote, seed_base, U256::one()),
                2,
                &key,
            )]),
            BlockContext::at_height(2),
        )
        .expect("pool with a one-unit quote side");
    let pair = derive_pair_id(&base, &quote, LP_FEE_BPS);
    let lp = derive_lp_asset(&pair);
    let shares = harness.db.asset_balance(&lp, &key.address()).expect("lp");
    let held = harness.db.asset_balance(&base, &key.address()).expect("b");
    let withdraw = TxKind::RemoveLiquidity(LiquidityWithdrawal {
        pair,
        shares,
        min_base: U256::one(),
        min_quote: U256::zero(),
    });
    harness
        .db
        .apply_block(
            &block_of(vec![signed(withdraw, 3, &key)]),
            BlockContext::at_height(3),
        )
        .expect("the base side is redeemable");
    assert!(
        harness
            .db
            .asset_balance(&lp, &key.address())
            .expect("lp")
            .is_zero()
    );
    assert!(harness.db.asset_balance(&base, &key.address()).expect("b") > held);
}
#[test]
fn the_same_pair_cannot_have_two_pools_at_one_fee_rate() {
    let mut market = market();
    let error = market
        .apply(
            vec![TxKind::CreatePool(PoolCreation {
                asset_a: market.asset,
                asset_b: NATIVE_ASSET,
                lp_fee_bps: LP_FEE_BPS,
                amount_a: primitive_types::U256::from(1_000),
                amount_b: primitive_types::U256::from(1_000),
            })],
            2,
        )
        .unwrap_err();

    assert!(matches!(error, NodeError::PoolExists(_)));
}

#[test]
fn the_pair_ordering_is_canonical_so_naming_the_assets_backwards_finds_the_same_pool() {
    // `PoolCreation` takes the assets in whatever order the sender wrote them.
    // If the canonicalisation were not applied to the *amounts* as well, this
    // would deposit at the inverted ratio.
    let mut market = market();
    let before = market.reserves();

    market
        .apply(
            vec![TxKind::AddLiquidity(LiquidityDeposit {
                pair: market.pair,
                base_desired: primitive_types::U256::from(1_000),
                quote_desired: primitive_types::U256::from(4_000),
                min_shares: primitive_types::U256::from(1),
            })],
            2,
        )
        .expect("deposit");

    let after = market.reserves();
    assert_eq!(after.0, before.0 + 1_000);
    assert_eq!(after.1, before.1 + 4_000);
}

#[test]
fn a_deposit_that_would_mint_fewer_shares_than_asked_for_is_refused() {
    let mut market = market();
    let error = market
        .apply(
            vec![TxKind::AddLiquidity(LiquidityDeposit {
                pair: market.pair,
                base_desired: primitive_types::U256::from(1_000),
                quote_desired: primitive_types::U256::from(4_000),
                min_shares: primitive_types::U256::from(u64::MAX),
            })],
            2,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        NodeError::SlippageExceeded {
            what: "liquidity shares",
            ..
        }
    ));
}

#[test]
fn depositing_and_withdrawing_returns_no_more_than_was_put_in() {
    let mut market = market();
    let native_before = market.native(&market.maker_address);
    let held_before = market.held(&market.maker_address);
    let lp = derive_lp_asset(&market.pair);
    let shares_before = market
        .db()
        .asset_balance(&lp, &market.maker_address)
        .expect("s");

    market
        .apply(
            vec![TxKind::AddLiquidity(LiquidityDeposit {
                pair: market.pair,
                base_desired: primitive_types::U256::from(10_000),
                quote_desired: primitive_types::U256::from(40_000),
                min_shares: primitive_types::U256::from(1),
            })],
            2,
        )
        .expect("deposit");

    let minted = market
        .db()
        .asset_balance(&lp, &market.maker_address)
        .expect("s")
        - shares_before;
    market
        .apply(
            vec![TxKind::RemoveLiquidity(LiquidityWithdrawal {
                pair: market.pair,
                shares: primitive_types::U256::from(u64::try_from(minted).expect("fits u64")),
                min_base: primitive_types::U256::from(1),
                min_quote: primitive_types::U256::from(1),
            })],
            3,
        )
        .expect("withdraw");

    assert!(market.native(&market.maker_address) <= native_before);
    assert!(market.held(&market.maker_address) <= held_before);
}

#[test]
fn nobody_can_redeem_shares_they_do_not_hold() {
    let market = market();
    let thief = generate_signing_key().expect("key");
    let harness_nonce = 0;

    let block = block_of(vec![signed(
        TxKind::RemoveLiquidity(LiquidityWithdrawal {
            pair: market.pair,
            shares: primitive_types::U256::from(1_000),
            min_base: primitive_types::U256::from(0),
            min_quote: primitive_types::U256::from(0),
        }),
        harness_nonce,
        &thief,
    )]);

    let error = market
        .db()
        .apply_block(&block, BlockContext::at_height(2))
        .unwrap_err();
    assert!(matches!(error, NodeError::InsufficientAssetBalance { .. }));
}

// ---------------------------------------------------------------------------
// swaps
// ---------------------------------------------------------------------------

#[test]
fn a_swap_settles_at_the_end_of_the_block_and_moves_both_balances() {
    let mut market = market();
    let native_before = market.native(&market.maker_address);
    let held_before = market.held(&market.maker_address);

    market
        .apply(
            vec![TxKind::Swap(SwapRequest {
                pair: market.pair,
                direction: Direction::BaseToQuote.tag(),
                amount_in: primitive_types::U256::from(1_000),
                min_out: primitive_types::U256::from(1),
                deadline: 0,
            })],
            2,
        )
        .expect("swap");

    assert_eq!(market.native(&market.maker_address), native_before - 1_000);
    assert!(market.held(&market.maker_address) > held_before);
    // The reserves moved by the same amounts, less the protocol's cut, which is
    // zero today.
    let (base, _) = market.reserves();
    assert_eq!(base, POOL_NATIVE + 1_000);
}

#[test]
fn a_swap_that_misses_its_bound_is_a_no_op_and_does_not_fail_the_block() {
    // The single most important behaviour in the subsystem. If this returned an
    // error, anybody could void a block by putting a losing swap in it.
    let mut market = market();
    let native_before = market.native(&market.maker_address);
    let held_before = market.held(&market.maker_address);
    let reserves_before = market.reserves();

    market
        .apply(
            vec![TxKind::Swap(SwapRequest {
                pair: market.pair,
                direction: Direction::BaseToQuote.tag(),
                amount_in: primitive_types::U256::from(1_000),
                min_out: primitive_types::U256::from(u64::MAX),
                deadline: 0,
            })],
            2,
        )
        .expect("the block must still be valid");

    assert_eq!(market.native(&market.maker_address), native_before);
    assert_eq!(market.held(&market.maker_address), held_before);
    assert_eq!(market.reserves(), reserves_before);
    // And the nonce advanced: the transaction happened, it just did nothing.
    assert_eq!(
        market
            .db()
            .get_account(&market.maker_address)
            .expect("a")
            .nonce,
        market.nonce
    );
}

#[test]
fn a_swap_past_its_deadline_is_refused() {
    // The defence against a held transaction: a miner who sits on a swap
    // cannot execute it against a market that has moved on.
    let mut market = market();
    let error = market
        .apply(
            vec![TxKind::Swap(SwapRequest {
                pair: market.pair,
                direction: Direction::BaseToQuote.tag(),
                amount_in: primitive_types::U256::from(1_000),
                min_out: primitive_types::U256::from(0),
                deadline: 5,
            })],
            6,
        )
        .unwrap_err();

    assert!(matches!(error, NodeError::DeadlineExpired { .. }));
}

#[test]
fn two_opposing_swaps_in_one_block_settle_at_one_price() {
    // The uniform-price claim, on chain. The two traders are on opposite sides,
    // so most of their volume crosses internally and never touches the curve —
    // and both of them get the same rate for it.
    let mut market = market();
    let other = generate_signing_key().expect("key");
    let other_address = other.address();

    // Give the second trader something to sell.
    market
        .apply(
            vec![TxKind::TransferAsset(AssetTransfer {
                asset: market.asset,
                recipient: other_address,
                amount: primitive_types::U256::from(40_000),
            })],
            2,
        )
        .expect("fund the counterparty");

    let seller_before = market.held(&other_address);
    let buyer_native_before = market.native(&market.maker_address);

    let block = block_of(vec![
        signed(
            TxKind::Swap(SwapRequest {
                pair: market.pair,
                direction: Direction::BaseToQuote.tag(),
                amount_in: primitive_types::U256::from(5_000),
                min_out: primitive_types::U256::from(1),
                deadline: 0,
            }),
            market.nonce,
            &market.maker,
        ),
        signed(
            TxKind::Swap(SwapRequest {
                pair: market.pair,
                direction: Direction::QuoteToBase.tag(),
                amount_in: primitive_types::U256::from(20_000),
                min_out: primitive_types::U256::from(1),
                deadline: 0,
            }),
            0,
            &other,
        ),
    ]);
    market
        .db()
        .apply_block(&block, BlockContext::at_height(3))
        .expect("batch");

    let buyer_received = market.held(&market.maker_address);
    let seller_received = market.native(&other_address);
    assert!(buyer_received > 0 && seller_received > 0);
    assert_eq!(
        market.native(&market.maker_address),
        buyer_native_before - 5_000
    );
    assert_eq!(market.held(&other_address), seller_before - 20_000);

    // Both settled at the same rate, so the ratio each got is the same to
    // within the unit that flooring a payout costs.
    let batch_price = market
        .db()
        .get_pool(&market.pair)
        .expect("read")
        .expect("pool");
    assert!(batch_price.pool.reserve_base > maya_ledger_math::Amount::from_u64(0));
}

#[test]
fn the_order_of_swaps_within_a_block_does_not_change_the_outcome() {
    // If it did, a miner would be able to extract value simply by reordering,
    // and every claim the batch makes would be void.
    let maker = generate_signing_key().expect("key");
    let seller = generate_signing_key().expect("key");
    let buyer = generate_signing_key().expect("key");

    let roots: Vec<[u8; 32]> = [false, true]
        .iter()
        .map(|reversed| {
            let mut market = market_with(maker.clone());
            market
                .apply(
                    vec![TxKind::TransferAsset(AssetTransfer {
                        asset: market.asset,
                        recipient: seller.address(),
                        amount: primitive_types::U256::from(50_000),
                    })],
                    2,
                )
                .expect("fund");
            market
                .db()
                .put_account(
                    &buyer.address(),
                    &Account {
                        balance: U256::from(50_000),
                        nonce: 0,
                    },
                )
                .expect("fund buyer");

            let mut transactions = vec![
                signed(
                    TxKind::Swap(SwapRequest {
                        pair: market.pair,
                        direction: Direction::QuoteToBase.tag(),
                        amount_in: primitive_types::U256::from(30_000),
                        min_out: primitive_types::U256::from(1),
                        deadline: 0,
                    }),
                    0,
                    &seller,
                ),
                signed(
                    TxKind::Swap(SwapRequest {
                        pair: market.pair,
                        direction: Direction::BaseToQuote.tag(),
                        amount_in: primitive_types::U256::from(7_000),
                        min_out: primitive_types::U256::from(1),
                        deadline: 0,
                    }),
                    0,
                    &buyer,
                ),
            ];
            if *reversed {
                transactions.reverse();
            }

            market
                .db()
                .apply_block(&block_of(transactions), BlockContext::at_height(3))
                .expect("batch")
        })
        .collect();

    assert_eq!(
        roots[0], roots[1],
        "reordering the block changed the resulting state"
    );
}

// ---------------------------------------------------------------------------
// order book
// ---------------------------------------------------------------------------

#[test]
fn placing_an_order_takes_its_escrow_and_cancelling_returns_it() {
    let mut market = market();
    let held_before = market.held(&market.maker_address);
    let order_id = custom_l1_node::state::dex::derive_order_id(&market.maker_address, market.nonce);

    market
        .apply(
            vec![TxKind::PlaceOrder(OrderPlacement {
                pair: market.pair,
                side: Side::Bid.tag(),
                price: maya_stateless_core::amount::to_u256(price(4)),
                amount: primitive_types::U256::from(1_000),
                expiry: 0,
            })],
            2,
        )
        .expect("place");

    // A bid escrows quote: 1000 base at four quote each.
    assert_eq!(market.held(&market.maker_address), held_before - 4_000);
    assert!(market.db().get_order(&order_id).expect("read").is_some());

    market
        .apply(vec![TxKind::CancelOrder(order_id)], 3)
        .expect("cancel");

    assert_eq!(market.held(&market.maker_address), held_before);
    assert!(market.db().get_order(&order_id).expect("read").is_none());
}

#[test]
fn an_order_worth_less_than_one_quote_unit_is_refused_at_placement() {
    // Price 1 is a billionth of a quote unit per base unit, so one base unit
    // is worth a zero quote: an order no trade could ever pay for, which would
    // otherwise rest at the front of the book and sit in every pass's way.
    let mut market = market();
    let dust = TxKind::PlaceOrder(OrderPlacement {
        pair: market.pair,
        side: Side::Ask.tag(),
        price: primitive_types::U256::one(),
        amount: primitive_types::U256::one(),
        expiry: 0,
    });

    assert!(market.apply(vec![dust], 2).is_err());
}

#[test]
fn nobody_but_an_orders_owner_can_cancel_it() {
    let mut market = market();
    let order_id = custom_l1_node::state::dex::derive_order_id(&market.maker_address, market.nonce);
    market
        .apply(
            vec![TxKind::PlaceOrder(OrderPlacement {
                pair: market.pair,
                side: Side::Bid.tag(),
                price: maya_stateless_core::amount::to_u256(price(4)),
                amount: primitive_types::U256::from(1_000),
                expiry: 0,
            })],
            2,
        )
        .expect("place");

    let thief = generate_signing_key().expect("key");
    let block = block_of(vec![signed(TxKind::CancelOrder(order_id), 0, &thief)]);
    let error = market
        .db()
        .apply_block(&block, BlockContext::at_height(3))
        .unwrap_err();

    assert!(matches!(error, NodeError::NotOrderOwner { .. }));
}

#[test]
fn two_crossing_orders_match_at_the_end_of_the_block() {
    let mut market = market();
    let taker = generate_signing_key().expect("key");
    let taker_address = taker.address();

    market
        .apply(
            vec![TxKind::TransferAsset(AssetTransfer {
                asset: market.asset,
                recipient: taker_address,
                amount: primitive_types::U256::from(100_000),
            })],
            2,
        )
        .expect("fund");

    let maker_held_before = market.held(&market.maker_address);
    let taker_native_before = market.native(&taker_address);

    // The maker rests an ask first, so it is the maker and sets the price.
    let block = block_of(vec![
        signed(
            TxKind::PlaceOrder(OrderPlacement {
                pair: market.pair,
                side: Side::Ask.tag(),
                price: maya_stateless_core::amount::to_u256(price(4)),
                amount: primitive_types::U256::from(1_000),
                expiry: 0,
            }),
            market.nonce,
            &market.maker,
        ),
        signed(
            TxKind::PlaceOrder(OrderPlacement {
                pair: market.pair,
                side: Side::Bid.tag(),
                price: maya_stateless_core::amount::to_u256(price(5)),
                amount: primitive_types::U256::from(1_000),
                expiry: 0,
            }),
            0,
            &taker,
        ),
    ]);
    market
        .db()
        .apply_block(&block, BlockContext::at_height(3))
        .expect("match");

    // The ask sold 1000 native for 4000 asset units at its own price, not at
    // the five the buyer was prepared to pay.
    assert_eq!(
        market.held(&market.maker_address),
        maker_held_before + 4_000
    );
    assert_eq!(market.native(&taker_address), taker_native_before + 1_000);

    // The buyer escrowed 5000 at their own price and only spent 4000. The
    // difference came back when the order left the book.
    assert!(market.held(&taker_address) >= 100_000 - 5_000 + 1_000);

    // Nothing is left resting.
    assert!(
        market
            .db()
            .get_order(&custom_l1_node::state::dex::derive_order_id(
                &taker_address,
                0
            ))
            .expect("read")
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// the governed protocol rate
// ---------------------------------------------------------------------------

/// Where governance keeps the parameter table (`PARAMETERS_KEY`, `g:params`).
const PARAMETERS_KEY: &[u8] = b"g:params";

/// Writes a protocol rate straight into the parameter table, as a passed
/// proposal leaves it. A full vote adds nothing to what these tests check.
fn govern_protocol_rate(market: &Market, bps: u64) {
    let table = ParameterTable::new().with(
        ParameterKey::DexProtocolFeeBps,
        maya_dex::Amount::from_u64(bps),
    );
    market
        .db()
        .raw_put_for_test(PARAMETERS_KEY, &table.encode())
        .expect("govern the rate");
}

fn swap_base_to_quote(market: &Market, amount: u64) -> TxKind {
    TxKind::Swap(SwapRequest {
        pair: market.pair,
        direction: Direction::BaseToQuote.tag(),
        amount_in: primitive_types::U256::from(amount),
        min_out: primitive_types::U256::from(1),
        deadline: 0,
    })
}

#[test]
fn a_governed_rate_the_pool_cannot_carry_refunds_the_batch_without_failing_the_block() {
    // A 400 bps LP rate plus a 200 bps governed rate is past the 500 bps
    // ceiling, so this pool cannot carry the governed rate.
    let mut market = market_with_fee(generate_signing_key().expect("key"), 400);
    govern_protocol_rate(&market, 200);
    let native_before = market.native(&market.maker_address);
    let reserves_before = market.reserves();

    market
        .apply(vec![swap_base_to_quote(&market, 1_000)], 2)
        .expect("the block still applies");

    // The input escrowed at submission comes straight back, and the pool
    // does not move.
    assert_eq!(market.native(&market.maker_address), native_before);
    assert_eq!(market.reserves(), reserves_before);
}

/// What a bidder takes from one crossed pair of orders on a pool with LP rate
/// `lp_fee_bps`, at governed protocol rate `rate`. The bidder starts with no
/// native coin, so this is the base they received from the fill.
fn bidder_receives(maker: &HybridSigningKey, lp_fee_bps: u32, rate: u64) -> u64 {
    let mut market = market_with_fee(maker.clone(), lp_fee_bps);
    govern_protocol_rate(&market, rate);
    let taker = generate_signing_key().expect("key");
    let taker_address = taker.address();
    market
        .apply(
            vec![TxKind::TransferAsset(AssetTransfer {
                asset: market.asset,
                recipient: taker_address,
                amount: primitive_types::U256::from(100_000),
            })],
            2,
        )
        .expect("fund");
    let ask_nonce = market.nonce;
    let block = block_of(vec![
        signed(
            TxKind::PlaceOrder(OrderPlacement {
                pair: market.pair,
                side: Side::Ask.tag(),
                price: maya_stateless_core::amount::to_u256(price(4)),
                amount: primitive_types::U256::from(1_000),
                expiry: 0,
            }),
            ask_nonce,
            &market.maker,
        ),
        signed(
            TxKind::PlaceOrder(OrderPlacement {
                pair: market.pair,
                side: Side::Bid.tag(),
                price: maya_stateless_core::amount::to_u256(price(5)),
                amount: primitive_types::U256::from(1_000),
                expiry: 0,
            }),
            0,
            &taker,
        ),
    ]);
    market
        .db()
        .apply_block(&block, BlockContext::at_height(3))
        .expect("the block applies");
    market.native(&taker_address)
}

#[test]
fn a_governed_rate_past_the_lp_ceiling_still_matches_the_book_at_the_protocol_rate() {
    // LP 400 + governed 200 is past the 500 bps ceiling for a swap, but a
    // book fill pays only the protocol rate, so the book must still match.
    let maker = generate_signing_key().expect("key");
    let untaxed = bidder_receives(&maker, 400, 0);
    let taxed = bidder_receives(&maker, 400, 200);
    assert!(taxed > 0, "the book did not match");
    assert!(taxed < untaxed, "the governed rate was not charged");
    // The LP rate never reaches a book trade.
    assert_eq!(taxed, bidder_receives(&maker, LP_FEE_BPS, 200));
}

#[test]
fn a_nonzero_governed_rate_takes_its_cut_of_a_batched_swap() {
    let maker = generate_signing_key().expect("key");
    let mut received = Vec::new();
    for rate in [0_u64, 100] {
        let mut market = market_with(maker.clone());
        govern_protocol_rate(&market, rate);
        market
            .apply(vec![swap_base_to_quote(&market, 1_000)], 2)
            .expect("swap");
        received.push(market.held(&market.maker_address));
    }
    // The same swap returns less once the chain takes its rate.
    assert!(received[1] < received[0]);
}

#[test]
fn a_nonzero_governed_rate_takes_its_cut_of_a_book_fill() {
    // The bidder takes the fill net of the fee, and the rate adds to the fee.
    let maker = generate_signing_key().expect("key");
    assert!(bidder_receives(&maker, LP_FEE_BPS, 100) < bidder_receives(&maker, LP_FEE_BPS, 0));
}

#[test]
fn a_route_through_a_pool_that_cannot_carry_the_governed_rate_is_a_no_op() {
    // As for a leg the curve cannot price: the route does not pay, nothing is
    // written, and the block stays valid.
    let mut market = market_with_fee(generate_signing_key().expect("key"), 400);
    govern_protocol_rate(&market, 200);
    let native_before = market.native(&market.maker_address);
    let reserves_before = market.reserves();

    market
        .apply(
            vec![TxKind::SwapRoute(SwapRoute {
                legs: vec![RouteLeg {
                    pair: market.pair,
                    direction: Direction::BaseToQuote.tag(),
                }],
                amount_in: primitive_types::U256::from(1_000),
                min_out: primitive_types::U256::from(1),
                deadline: 0,
            })],
            2,
        )
        .expect("the block must still be valid");

    assert_eq!(market.native(&market.maker_address), native_before);
    assert_eq!(market.reserves(), reserves_before);
}

// ---------------------------------------------------------------------------
// routes and flash arbitrage
// ---------------------------------------------------------------------------

#[test]
fn an_atomic_route_across_two_mispriced_pools_captures_the_difference() {
    // The flash-arbitrage case. Two pools trade the same pair at different fee
    // rates and therefore have independent reserves; seeding them at different
    // ratios creates a real price difference, and a two-leg route closes it in
    // one transaction or not at all.
    let mut market = market();
    let cheap_pair = derive_pair_id(&NATIVE_ASSET, &market.asset, 100);

    market
        .apply(
            vec![TxKind::CreatePool(PoolCreation {
                asset_a: NATIVE_ASSET,
                asset_b: market.asset,
                lp_fee_bps: 100,
                // Half the price of the first pool.
                amount_a: primitive_types::U256::from(100_000),
                amount_b: primitive_types::U256::from(200_000),
            })],
            2,
        )
        .expect("second pool");

    let before = market.held(&market.maker_address);

    market
        .apply(
            vec![TxKind::SwapRoute(SwapRoute {
                legs: vec![
                    // Buy the asset where it is cheap...
                    RouteLeg {
                        pair: market.pair,
                        direction: Direction::BaseToQuote.tag(),
                    },
                    // ...and sell it back where it is dear.
                    RouteLeg {
                        pair: cheap_pair,
                        direction: Direction::QuoteToBase.tag(),
                    },
                ],
                amount_in: primitive_types::U256::from(5_000),
                min_out: primitive_types::U256::from(1),
                deadline: 0,
            })],
            3,
        )
        .expect("route");

    // The asset holding is unchanged — the route started and ended in native —
    // but both pools moved toward each other.
    assert_eq!(market.held(&market.maker_address), before);
    let dear = market.db().get_pool(&market.pair).expect("r").expect("p");
    let cheap = market.db().get_pool(&cheap_pair).expect("r").expect("p");
    assert!(dear.pool.reserve_base > maya_ledger_math::Amount::from_u64(POOL_NATIVE));
    assert!(cheap.pool.reserve_base < maya_ledger_math::Amount::from_u64(100_000));
}

#[test]
fn a_route_that_does_not_clear_its_bound_leaves_no_trace() {
    // Two arbitrageurs racing one opportunity is the normal case. The loser's
    // transaction must be a no-op, not a block-invalidating error, and it must
    // not have written half a route.
    let mut market = market();
    let cheap_pair = derive_pair_id(&NATIVE_ASSET, &market.asset, 100);
    market
        .apply(
            vec![TxKind::CreatePool(PoolCreation {
                asset_a: NATIVE_ASSET,
                asset_b: market.asset,
                lp_fee_bps: 100,
                amount_a: primitive_types::U256::from(100_000),
                amount_b: primitive_types::U256::from(400_000),
            })],
            2,
        )
        .expect("second pool");

    let native_before = market.native(&market.maker_address);
    let dear_before = market.reserves();

    market
        .apply(
            vec![TxKind::SwapRoute(SwapRoute {
                legs: vec![
                    RouteLeg {
                        pair: market.pair,
                        direction: Direction::BaseToQuote.tag(),
                    },
                    RouteLeg {
                        pair: cheap_pair,
                        direction: Direction::QuoteToBase.tag(),
                    },
                ],
                amount_in: primitive_types::U256::from(5_000),
                min_out: primitive_types::U256::from(u64::MAX),
                deadline: 0,
            })],
            3,
        )
        .expect("the block must still be valid");

    assert_eq!(market.native(&market.maker_address), native_before);
    assert_eq!(market.reserves(), dear_before, "the first leg was written");
}

#[test]
fn a_round_trip_through_one_pool_is_refused_rather_than_priced_twice() {
    // A route used to be allowed to revisit a pool, the second visit priced
    // against the first. Every pool now clears once per block (ADR-154), so a
    // revisiting route could never execute, and it is refused as malformed.
    let mut market = market();
    let error = market
        .apply(
            vec![TxKind::SwapRoute(SwapRoute {
                legs: vec![
                    RouteLeg {
                        pair: market.pair,
                        direction: Direction::BaseToQuote.tag(),
                    },
                    RouteLeg {
                        pair: market.pair,
                        direction: Direction::QuoteToBase.tag(),
                    },
                ],
                amount_in: primitive_types::U256::from(10_000),
                min_out: primitive_types::U256::from(1),
                deadline: 0,
            })],
            2,
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            NodeError::Trade {
                operation: "route",
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn a_route_whose_legs_do_not_join_up_is_rejected() {
    // Malformed, not merely unprofitable: executing it would leave the sender
    // holding an asset they never asked for.
    let mut market = market();
    let error = market
        .apply(
            vec![TxKind::SwapRoute(SwapRoute {
                legs: vec![
                    RouteLeg {
                        pair: market.pair,
                        direction: Direction::BaseToQuote.tag(),
                    },
                    // Ends in the asset, then tries to sell the asset again.
                    RouteLeg {
                        pair: market.pair,
                        direction: Direction::BaseToQuote.tag(),
                    },
                ],
                amount_in: primitive_types::U256::from(1_000),
                min_out: primitive_types::U256::from(0),
                deadline: 0,
            })],
            2,
        )
        .unwrap_err();

    assert!(matches!(error, NodeError::RouteDiscontinuity { leg: 1 }));
}

// ---------------------------------------------------------------------------
// reorgs and the state root
// ---------------------------------------------------------------------------

#[test]
fn reverting_a_block_restores_the_pool_it_traded_against() {
    // Without the undo journal covering trading records, a reorg leaves the
    // pool holding the abandoned chain's reserves — two plausible numbers that
    // go on quoting a price nobody agreed to.
    let market = market();
    let before = market.reserves();
    let root_before = market.db().state_root().expect("root");

    let mut block = block_of(vec![signed(
        TxKind::Swap(SwapRequest {
            pair: market.pair,
            direction: Direction::BaseToQuote.tag(),
            amount_in: primitive_types::U256::from(20_000),
            min_out: primitive_types::U256::from(1),
            deadline: 0,
        }),
        market.nonce,
        &market.maker,
    )]);
    block.header.state_root = market
        .db()
        .preview_root(&block, BlockContext::at_height(3))
        .expect("preview");
    let block_id = [9u8; 32];
    market
        .db()
        .apply_block_journaled(&block, &block_id, BlockContext::at_height(3))
        .expect("apply");
    assert_eq!(
        market.db().uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "every stored key must be under the state root or declared local-only"
    );

    assert_ne!(market.reserves(), before, "the swap did nothing to revert");

    market.db().revert_block(&block_id).expect("revert");

    assert_eq!(market.reserves(), before);
    assert_eq!(market.db().state_root().expect("root"), root_before);
}

#[test]
fn reverting_a_block_that_created_a_pool_removes_it_entirely() {
    // An account that did not exist before a block is deleted rather than
    // restored as a zero. A pool is the same problem: leaving an empty record
    // behind would change the state root.
    let market = market();
    let root_before = market.db().state_root().expect("root");
    let new_pair = derive_pair_id(&NATIVE_ASSET, &market.asset, 100);

    let mut block = block_of(vec![signed(
        TxKind::CreatePool(PoolCreation {
            asset_a: NATIVE_ASSET,
            asset_b: market.asset,
            lp_fee_bps: 100,
            amount_a: primitive_types::U256::from(10_000),
            amount_b: primitive_types::U256::from(40_000),
        }),
        market.nonce,
        &market.maker,
    )]);
    block.header.state_root = market
        .db()
        .preview_root(&block, BlockContext::at_height(3))
        .expect("preview");
    let block_id = [11u8; 32];
    market
        .db()
        .apply_block_journaled(&block, &block_id, BlockContext::at_height(3))
        .expect("apply");
    assert!(market.db().get_pool(&new_pair).expect("read").is_some());

    market.db().revert_block(&block_id).expect("revert");

    assert!(market.db().get_pool(&new_pair).expect("read").is_none());
    assert_eq!(market.db().state_root().expect("root"), root_before);
}

#[test]
fn a_chain_that_has_never_traded_has_the_state_root_it_always_had() {
    // The trading layer folds into the root only when there is something in it,
    // so an existing chain's committed roots stay exactly what they were.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);

    let expected = custom_l1_node::state::merkle_root(&[custom_l1_node::state::account_leaf(
        &key.address(),
        &Account {
            balance: U256::from(1_000),
            nonce: 0,
        },
    )]);

    assert_eq!(harness.db.state_root().expect("root"), expected);
}

#[test]
fn registering_an_asset_changes_the_state_root() {
    // The other half of the previous test: the layer must actually be committed
    // to, or a light client could not verify a balance in it.
    let key = generate_signing_key().expect("key");
    let harness = fixture(&[(key.address(), 1_000)]);
    let before = harness.db.state_root().expect("root");

    let block = block_of(vec![signed(
        TxKind::RegisterAsset(AssetRegistration {
            symbol: symbol("AAA"),
            total_supply: primitive_types::U256::from(500),
        }),
        0,
        &key,
    )]);
    harness
        .db
        .apply_block(&block, BlockContext::at_height(1))
        .expect("apply");

    assert_ne!(harness.db.state_root().expect("root"), before);
}
