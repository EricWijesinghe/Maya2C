//! Uniform-price batch clearing, and the sandwich it is there to stop.

use maya_dex::amm::Pool;
use maya_dex::batch::{MAX_BATCH_INTENTS, SwapIntent, clear_batch};
use maya_dex::error::DexError;
use maya_dex::fees::FeeSchedule;
use maya_dex::types::Direction;

fn seeded(base: u64, quote: u64, fees: FeeSchedule) -> Pool {
    Pool::empty(fees)
        .add_liquidity(base, quote)
        .expect("seed")
        .pool
}

fn intent(id: u8, direction: Direction, amount_in: u64, min_out: u64) -> SwapIntent {
    SwapIntent {
        id: [id; 32],
        trader: [id; 32],
        direction,
        amount_in,
        min_out,
    }
}

const FREE: FeeSchedule = FeeSchedule {
    lp_bps: 0,
    protocol_bps: 0,
};

#[test]
fn everyone_in_a_batch_settles_at_the_same_price() {
    let pool = seeded(1_000_000, 1_000_000, FeeSchedule::standard());
    let intents = [
        intent(1, Direction::BaseToQuote, 10_000, 0),
        intent(2, Direction::BaseToQuote, 50_000, 0),
        intent(3, Direction::QuoteToBase, 20_000, 0),
    ];

    let outcome = clear_batch(&pool, &intents).expect("clear");
    assert!(outcome.skipped.is_empty());

    // Not "approximately the same price" — exactly the batch price, floored.
    // An approximate check would pass a design that quietly gave large traders
    // a better rate, which is the thing being ruled out.
    let scale = 1_000_000_000_u128;
    for trade in &outcome.cleared {
        let expected = match trade.direction {
            Direction::BaseToQuote => (trade.amount_in as u128) * (outcome.price as u128) / scale,
            Direction::QuoteToBase => (trade.amount_in as u128) * scale / (outcome.price as u128),
        };
        assert_eq!(
            trade.amount_out as u128, expected,
            "trade {} did not settle at the batch price",
            trade.id[0]
        );
    }
}

#[test]
fn a_sandwich_that_profits_when_executed_in_sequence_does_not_profit_in_a_batch() {
    // The headline claim. Run the same three trades twice: once as a sequence
    // of individual swaps, which is what a curve without batching does, and
    // once as a batch.
    let pool = seeded(1_000_000, 1_000_000, FREE);
    let victim_in = 50_000_u64;
    let attack_in = 100_000_u64;

    // Sequential: buy in front of the victim, sell behind them.
    let front = pool
        .swap_exact_in(Direction::QuoteToBase, attack_in)
        .expect("front-run");
    let victim_sequential = front
        .pool
        .swap_exact_in(Direction::QuoteToBase, victim_in)
        .expect("victim");
    let back = victim_sequential
        .pool
        .swap_exact_in(Direction::BaseToQuote, front.amount_out)
        .expect("back-run");

    let sequential_profit = back.amount_out as i128 - attack_in as i128;
    assert!(
        sequential_profit > 0,
        "the sandwich must actually pay when executed in sequence, \
         or this test proves nothing: profit was {sequential_profit}"
    );

    // Batched: the same three trades, one price. The attacker's two legs are in
    // the same batch as the victim's, so the price they engineered is the price
    // they get.
    let batch = clear_batch(
        &pool,
        &[
            intent(1, Direction::QuoteToBase, attack_in, 0),
            intent(2, Direction::QuoteToBase, victim_in, 0),
            intent(3, Direction::BaseToQuote, front.amount_out, 0),
        ],
    )
    .expect("clear");

    let attacker_base_bought = batch.cleared[0].amount_out;
    let attacker_quote_recovered = batch.cleared[2].amount_out;
    let attacker_base_sold = batch.cleared[2].amount_in;

    // The attacker put in `attack_in` quote and `attacker_base_sold` base, and
    // took out `attacker_quote_recovered` quote and `attacker_base_bought`
    // base. Profitable only if they are ahead on both.
    let ahead_on_quote = attacker_quote_recovered > attack_in;
    let ahead_on_base = attacker_base_bought > attacker_base_sold;
    assert!(
        !(ahead_on_quote && ahead_on_base),
        "the sandwich paid inside the batch: quote {attack_in} -> \
         {attacker_quote_recovered}, base {attacker_base_sold} -> \
         {attacker_base_bought}"
    );

    // And the victim is better off than they were under sequencing.
    assert!(
        batch.cleared[1].amount_out > victim_sequential.amount_out,
        "batching did not improve the victim's fill: {} vs {}",
        batch.cleared[1].amount_out,
        victim_sequential.amount_out
    );
}

#[test]
fn a_balanced_batch_clears_at_spot_and_charges_nothing() {
    // Nothing reaches the curve, so the pool lent no capital and takes no fee.
    // A perfectly balanced batch is traders trading with each other.
    let pool = seeded(1_000_000, 1_000_000, FeeSchedule::standard());
    let spot = pool.spot_price().expect("spot");

    let outcome = clear_batch(
        &pool,
        &[
            intent(1, Direction::BaseToQuote, 10_000, 0),
            intent(2, Direction::QuoteToBase, 10_000, 0),
        ],
    )
    .expect("clear");

    assert_eq!(outcome.price, spot);
    assert_eq!(outcome.protocol_fee, 0);
    assert_eq!(outcome.protocol_fee_direction, None);
    assert_eq!(outcome.cleared[0].amount_out, 10_000);
    assert_eq!(outcome.cleared[1].amount_out, 10_000);
}

#[test]
fn the_result_does_not_depend_on_the_order_the_intents_arrive_in() {
    // If it did, a miner would be back in business simply by reordering, and
    // every claim in this module would be void.
    let pool = seeded(2_000_000, 3_000_000, FeeSchedule::standard());
    let mut intents = vec![
        intent(1, Direction::BaseToQuote, 11_111, 0),
        intent(2, Direction::QuoteToBase, 70_000, 0),
        intent(3, Direction::BaseToQuote, 3, 0),
        intent(4, Direction::QuoteToBase, 999, 0),
        intent(5, Direction::BaseToQuote, 250_000, 0),
    ];

    let baseline = clear_batch(&pool, &intents).expect("clear");

    // Every rotation is a different miner's ordering.
    for _ in 0..intents.len() {
        intents.rotate_left(1);
        let permuted = clear_batch(&pool, &intents).expect("clear");

        assert_eq!(permuted.price, baseline.price);
        assert_eq!(permuted.pool, baseline.pool);
        for trade in &permuted.cleared {
            let matching = baseline
                .cleared
                .iter()
                .find(|t| t.id == trade.id)
                .expect("same set of trades");
            assert_eq!(trade.amount_out, matching.amount_out);
        }
    }
}

#[test]
fn an_intent_that_misses_its_bound_is_skipped_and_the_rest_still_clear() {
    // The single most important behaviour in the module: a missed slippage
    // bound is a no-op, not an error. If it were an error it would abort block
    // execution, and one trader's bound would be a weapon against every block
    // that carried it.
    let pool = seeded(1_000_000, 1_000_000, FeeSchedule::standard());

    let outcome = clear_batch(
        &pool,
        &[
            intent(1, Direction::BaseToQuote, 100_000, 0),
            // Demands more than any price this batch could produce.
            intent(2, Direction::BaseToQuote, 10_000, u64::MAX),
            intent(3, Direction::QuoteToBase, 5_000, 0),
        ],
    )
    .expect("clear");

    assert_eq!(outcome.skipped, vec![[2u8; 32]]);
    assert_eq!(outcome.cleared.len(), 2);
    assert!(outcome.cleared.iter().all(|t| t.id != [2u8; 32]));
}

#[test]
fn dropping_one_intent_can_drop_another_and_the_loop_still_terminates() {
    // Removing a seller raises the price for the sellers who remain and lowers
    // it for the buyers, so a removal can push a *buyer* under their bound. The
    // set is shrunk to a fixed point rather than in one pass.
    let pool = seeded(1_000_000, 1_000_000, FREE);

    // Price the batch once to find bounds that sit just on the edge.
    let reference = clear_batch(
        &pool,
        &[
            intent(1, Direction::BaseToQuote, 200_000, 0),
            intent(2, Direction::QuoteToBase, 10_000, 0),
        ],
    )
    .expect("reference");
    let buyer_out = reference.cleared[1].amount_out;

    let outcome = clear_batch(
        &pool,
        &[
            intent(1, Direction::BaseToQuote, 200_000, u64::MAX),
            // Only achievable while the big seller is in the batch pushing the
            // price down.
            intent(2, Direction::QuoteToBase, 10_000, buyer_out),
        ],
    )
    .expect("clear");

    assert_eq!(outcome.skipped.len(), 2, "both dropped, in cascade");
    assert!(outcome.cleared.is_empty());
    assert_eq!(outcome.pool, pool, "a fully skipped batch touches nothing");
}

#[test]
fn a_batch_never_lowers_the_invariant() {
    let pool = seeded(5_000_000, 7_000_000, FeeSchedule::standard());

    for (sell, buy) in [
        (1_u64, 1_u64),
        (100, 900_000),
        (900_000, 100),
        (123_456, 654_321),
        (1, 1_000_000),
    ] {
        let outcome = clear_batch(
            &pool,
            &[
                intent(1, Direction::BaseToQuote, sell, 0),
                intent(2, Direction::QuoteToBase, buy, 0),
            ],
        )
        .expect("clear");

        assert!(
            outcome.pool.k() >= pool.k(),
            "k fell on ({sell}, {buy}): {} -> {}",
            pool.k(),
            outcome.pool.k()
        );
    }
}

#[test]
fn every_unit_supplied_is_accounted_for_by_a_payout_the_pool_or_the_sink() {
    let fees = FeeSchedule::new(30, 20).expect("fees");
    let pool = seeded(4_000_000, 1_000_000, fees);
    let intents = [
        intent(1, Direction::BaseToQuote, 300_000, 0),
        intent(2, Direction::BaseToQuote, 17, 0),
        intent(3, Direction::QuoteToBase, 40_000, 0),
    ];

    let outcome = clear_batch(&pool, &intents).expect("clear");

    let (mut base_in, mut quote_in) = (0u128, 0u128);
    let (mut base_out, mut quote_out) = (0u128, 0u128);
    for trade in &outcome.cleared {
        match trade.direction {
            Direction::BaseToQuote => {
                base_in += trade.amount_in as u128;
                quote_out += trade.amount_out as u128;
            }
            Direction::QuoteToBase => {
                quote_in += trade.amount_in as u128;
                base_out += trade.amount_out as u128;
            }
        }
    }

    let sink_base = match outcome.protocol_fee_direction {
        Some(Direction::BaseToQuote) => outcome.protocol_fee as u128,
        _ => 0,
    };
    let sink_quote = match outcome.protocol_fee_direction {
        Some(Direction::QuoteToBase) => outcome.protocol_fee as u128,
        _ => 0,
    };

    assert_eq!(
        pool.reserve_base as u128 + base_in,
        outcome.pool.reserve_base as u128 + base_out + sink_base,
        "base is not conserved"
    );
    assert_eq!(
        pool.reserve_quote as u128 + quote_in,
        outcome.pool.reserve_quote as u128 + quote_out + sink_quote,
        "quote is not conserved"
    );
}

#[test]
fn shares_are_untouched_by_trading() {
    let pool = seeded(1_000_000, 1_000_000, FeeSchedule::standard());
    let outcome =
        clear_batch(&pool, &[intent(1, Direction::BaseToQuote, 90_000, 0)]).expect("clear");
    assert_eq!(outcome.pool.total_shares, pool.total_shares);
    // But each share is worth more, which is how the providers were paid.
    assert!(outcome.pool.k() > pool.k());
}

#[test]
fn a_batch_larger_than_the_ceiling_is_refused() {
    let pool = seeded(1_000_000_000, 1_000_000_000, FREE);
    let intents: Vec<SwapIntent> = (0..=MAX_BATCH_INTENTS)
        .map(|i| SwapIntent {
            id: [i as u8; 32],
            trader: [i as u8; 32],
            direction: Direction::BaseToQuote,
            amount_in: 1_000,
            min_out: 0,
        })
        .collect();

    assert_eq!(
        clear_batch(&pool, &intents).err(),
        Some(DexError::WorkLimitReached)
    );
}

#[test]
fn an_unfunded_pool_clears_nothing() {
    let pool = Pool::empty(FeeSchedule::standard());
    assert_eq!(
        clear_batch(&pool, &[intent(1, Direction::BaseToQuote, 10, 0)]).err(),
        Some(DexError::EmptyPool)
    );
}

#[test]
fn a_zero_amount_intent_is_rejected_before_anything_is_priced() {
    let pool = seeded(1_000_000, 1_000_000, FREE);
    assert_eq!(
        clear_batch(&pool, &[intent(1, Direction::BaseToQuote, 0, 0)]).err(),
        Some(DexError::ZeroAmount)
    );
}

#[test]
fn an_empty_batch_leaves_the_pool_exactly_as_it_was() {
    let pool = seeded(1_000_000, 3_000_000, FeeSchedule::standard());
    let outcome = clear_batch(&pool, &[]).expect("clear");

    assert_eq!(outcome.pool, pool);
    assert_eq!(outcome.price, pool.spot_price().expect("spot"));
    assert!(outcome.cleared.is_empty());
}

#[test]
fn a_lone_swap_in_a_batch_is_priced_no_better_than_the_curve_would_have() {
    // With nothing to net against, the batch degenerates to a plain swap. It
    // must not accidentally become a *cheaper* one, or a trader would split
    // their order to game it.
    let pool = seeded(1_000_000, 1_000_000, FeeSchedule::standard());
    let direct = pool
        .swap_exact_in(Direction::BaseToQuote, 75_000)
        .expect("swap");
    let batched =
        clear_batch(&pool, &[intent(1, Direction::BaseToQuote, 75_000, 0)]).expect("clear");

    assert!(
        batched.cleared[0].amount_out <= direct.amount_out,
        "batching a single swap paid more than the curve: {} vs {}",
        batched.cleared[0].amount_out,
        direct.amount_out
    );
}
