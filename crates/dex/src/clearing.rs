//! Block clearing: every pool clears exactly once per block, routes included.
//!
//! # The guarantee
//!
//! Within one block, no trade on a pool executes before another trade on the
//! same pool. Plain swaps and every leg of every multi-hop route meet in that
//! pool's single uniform-price batch ([`crate::batch`]), so a trade cannot be
//! placed in front of a route, behind it, or around it: there is no "in
//! front". A sandwich needs two executions on one pool with the victim between
//! them, and a pool here has one (ADR-154).
//!
//! # How a route fits into one clearing per pool
//!
//! A route's second leg needs its first leg's output, so its first pool must
//! clear before its second. Each admitted route therefore adds the edges
//! `hop[i].pair -> hop[i + 1].pair` to a precedence graph, and pools clear in
//! a topological order of that graph — smallest pair identifier first among
//! the pools that are ready, so the order is a property of the set of
//! intents and never of where a transaction sat in the block.
//!
//! Two routes can ask for opposite orders (`P -> Q` and `Q -> P`). Both cannot
//! be honoured while each pool clears once, so the later of the two in
//! priority order (below) is a **no-op**: its input is returned and nothing
//! moves (invariant 7). A route that visits one pool twice closes a cycle with
//! itself; the node refuses it at staging, because it could never execute.
//!
//! # All or nothing, across pools, in fee order
//!
//! A route is atomic: either every leg settles and its final output meets
//! `min_out`, or the route is refunded whole. Routes join the clearing one at
//! a time in priority order: the fee their transaction paid, highest first,
//! then the smaller identifier, the same weight a full batch evicts by
//! ([`RouteIntent::weight`]). A route joins only if its path keeps the pool
//! order acyclic and the whole set, itself included, then settles in a
//! clearing recomputed from the original pools. Otherwise it is refunded and
//! the set stays as it was.
//!
//! Two guarantees follow:
//!
//! - **A refunded route leaves no trace.** The block clears exactly the set
//!   that joined, so a route that misses its bound, on purpose or not, cannot
//!   have moved a pool another route settled against, and cannot have kept
//!   another route out.
//! - **Only a route ahead of it that settles can keep a route out.** A route
//!   that pays less, or one that fails, never displaces it. A ground
//!   identifier wins only a tie between equal fees.
//!
//! There are at most `routes` passes, each bounded by [`MAX_BLOCK_ROUTES`] and
//! [`crate::batch::MAX_CLEARING_INTENTS`].
//!
//! Pools no route touches cannot be affected by a route's removal and are
//! cleared once, outside the loop.
//!
//! # What it does not solve
//!
//! The same limits as [`crate::batch`]: a miner spanning consecutive blocks,
//! and censorship. A trader who joins the same batch as a victim gets the
//! victim's price, not a better one; that is the batch working.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use maya_amount::Amount;

use crate::amm::Pool;
use crate::batch::{BatchOutcome, MAX_BATCH_INTENTS, SwapIntent, clear_batch};
use crate::error::{DexError, Result};
use crate::types::{Direction, PairId};

/// Most routes one block may carry.
///
/// Each route can cost one recomputation of every route-carrying pool, so the
/// bound is on the fixed point's passes. A compiled bound, liftable by a
/// scheduled upgrade keyed by height rather than a hard fork (ADR-154).
pub const MAX_BLOCK_ROUTES: usize = 64;

/// Most hops one route may take. The node's `MAX_ROUTE_LEGS` is this value.
pub const MAX_ROUTE_HOPS: usize = 4;

/// One leg of a route: which pool, which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteHop {
    /// Pool traded against.
    pub pair: PairId,
    /// Direction through that pool.
    pub direction: Direction,
}

/// A multi-hop swap waiting for the block's clearing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteIntent {
    /// Identifier, derived by the node from the submitting transaction.
    pub id: [u8; 32],
    /// Address the output is paid to.
    pub trader: [u8; 32],
    /// The path, in order. The node checks that the assets join up.
    pub hops: Vec<RouteHop>,
    /// Units of the first hop's input supplied.
    pub amount_in: Amount,
    /// Least the route will accept from its last hop.
    pub min_out: Amount,
    /// Priority: the fee its transaction paid. Higher joins the clearing
    /// first; equal weights fall back to the smaller identifier.
    pub weight: Amount,
}

/// What one pool's clearing produced, route legs removed.
#[derive(Clone, Debug)]
pub struct PoolClearing {
    /// The pool.
    pub pair: PairId,
    /// `None` when the pool could not clear its plain swaps at all — it is
    /// missing, unfunded, or every price it could reach is unusable. The
    /// caller refunds every plain swap on it. `Some` lists only plain swaps in
    /// `cleared` and `skipped`; route legs are reported in
    /// [`BlockClearing::routes`].
    pub outcome: Option<BatchOutcome>,
}

/// How one route ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteSettlement {
    /// The route.
    pub id: [u8; 32],
    /// Its trader.
    pub trader: [u8; 32],
    /// Units of the last hop's output paid, or `None` if the route is refunded.
    pub amount_out: Option<Amount>,
}

/// The whole block's trading outcome.
#[derive(Clone, Debug)]
pub struct BlockClearing {
    /// Every pool that had intents, in the order it cleared.
    pub pools: Vec<PoolClearing>,
    /// Every route, in identifier order.
    pub routes: Vec<RouteSettlement>,
}

/// A pool after clearing, if its price is still usable.
fn usable(outcome: BatchOutcome) -> Option<BatchOutcome> {
    let pool = outcome.pool;
    (!pool.is_funded() || pool.spot_price().is_ok_and(|price| !price.is_zero())).then_some(outcome)
}

/// Clears one pool's intents, or `None` if it cannot clear them.
fn clear_one(
    pools: &BTreeMap<PairId, Pool>,
    pair: &PairId,
    intents: &[SwapIntent],
) -> Option<BatchOutcome> {
    let pool = pools.get(pair)?;
    clear_batch(pool, intents).ok().and_then(usable)
}

/// Clears every pool and route of one block.
///
/// `pools` are the priced pools the caller can trade against; an intent on a
/// pool absent from it cannot clear. The result does not depend on the order
/// of `routes` or of the intents within `swaps`. Routes join in weight order,
/// each only if the whole set still settles (module documentation).
///
/// # Errors
///
/// - [`DexError::WorkLimitReached`] for more than [`MAX_BLOCK_ROUTES`] routes,
///   a route of more than [`MAX_ROUTE_HOPS`] hops, or a pair carrying more than
///   [`MAX_BATCH_INTENTS`] plain swaps.
/// - [`DexError::ZeroAmount`] for a route with no hops or no input.
pub fn clear_block(
    pools: &BTreeMap<PairId, Pool>,
    swaps: &BTreeMap<PairId, Vec<SwapIntent>>,
    routes: &[RouteIntent],
) -> Result<BlockClearing> {
    if routes.len() > MAX_BLOCK_ROUTES || swaps.values().any(|s| s.len() > MAX_BATCH_INTENTS) {
        return Err(DexError::WorkLimitReached);
    }
    for route in routes {
        if route.hops.len() > MAX_ROUTE_HOPS {
            return Err(DexError::WorkLimitReached);
        }
        if route.hops.is_empty() || route.amount_in.is_zero() {
            return Err(DexError::ZeroAmount);
        }
    }

    let mut order: Vec<&RouteIntent> = routes.iter().collect();
    order.sort_by(|x, y| y.weight.cmp(&x.weight).then(x.id.cmp(&y.id)));

    // Routes join one at a time, in priority order, and stay only if the
    // whole set still settles. A refused route is out of every later pass,
    // so it can neither move a pool nor fix an order (ADR-154).
    let mut live = alloc::vec![false; order.len()];
    let mut settled = Pass {
        pools: Vec::new(),
        amounts: Vec::new(),
    };
    for index in 0..order.len() {
        if !well_formed(pools, order[index]) {
            continue;
        }
        live[index] = true;
        let pass = schedule(&order, &live)
            .and_then(|schedule| run_pass(pools, swaps, &order, &live, &schedule));
        match pass {
            Some(pass) => settled = pass,
            None => live[index] = false,
        }
    }
    let route_pools = pools_of(&order, &live);
    let Pass {
        pools: mut cleared,
        amounts,
    } = settled;

    // Pools no route touches, once. Their order among themselves is pair order.
    for (pair, intents) in swaps {
        if !route_pools.contains(pair) && !intents.is_empty() {
            cleared.push(PoolClearing {
                pair: *pair,
                outcome: clear_one(pools, pair, intents),
            });
        }
    }

    let mut routes: Vec<RouteSettlement> = order
        .iter()
        .enumerate()
        .map(|(index, route)| RouteSettlement {
            id: route.id,
            trader: route.trader,
            amount_out: live[index].then(|| amounts[index]),
        })
        .collect();
    routes.sort_by_key(|settled| settled.id);
    Ok(BlockClearing {
        pools: cleared,
        routes,
    })
}

/// A route every pool of which the caller can price, visiting each once.
fn well_formed(pools: &BTreeMap<PairId, Pool>, route: &RouteIntent) -> bool {
    let distinct: BTreeSet<PairId> = route.hops.iter().map(|hop| hop.pair).collect();
    distinct.len() == route.hops.len() && route.hops.iter().all(|hop| pools.contains_key(&hop.pair))
}

/// Every pool a live route crosses.
fn pools_of(order: &[&RouteIntent], live: &[bool]) -> BTreeSet<PairId> {
    order
        .iter()
        .zip(live)
        .filter(|(_, live)| **live)
        .flat_map(|(route, _)| route.hops.iter().map(|hop| hop.pair))
        .collect()
}

/// The order the live routes' pools clear in, or `None` if their paths
/// ask for a cycle.
fn schedule(order: &[&RouteIntent], live: &[bool]) -> Option<Vec<PairId>> {
    let mut graph: BTreeMap<PairId, BTreeSet<PairId>> = BTreeMap::new();
    for (route, _) in order.iter().zip(live).filter(|(_, live)| **live) {
        for window in route.hops.windows(2) {
            graph
                .entry(window[0].pair)
                .or_default()
                .insert(window[1].pair);
        }
    }
    let nodes = pools_of(order, live);
    let sorted = topological(&graph, &nodes);
    (sorted.len() == nodes.len()).then_some(sorted)
}

/// Kahn's algorithm over `nodes`, smallest ready identifier first. A cycle
/// leaves its nodes out, which is how [`schedule`] detects one.
fn topological(
    graph: &BTreeMap<PairId, BTreeSet<PairId>>,
    nodes: &BTreeSet<PairId>,
) -> Vec<PairId> {
    let mut indegree: BTreeMap<PairId, usize> = nodes.iter().map(|node| (*node, 0)).collect();
    for targets in graph.values() {
        for target in targets {
            if let Some(count) = indegree.get_mut(target) {
                *count += 1;
            }
        }
    }
    let mut ready: BTreeSet<PairId> = indegree
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(node, _)| *node)
        .collect();
    let mut out = Vec::with_capacity(nodes.len());
    while let Some(node) = ready.pop_first() {
        out.push(node);
        for target in graph.get(&node).into_iter().flatten() {
            if let Some(count) = indegree.get_mut(target) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(*target);
                }
            }
        }
    }
    out
}

/// One pass over the route-carrying pools in which every live route settled.
struct Pass {
    pools: Vec<PoolClearing>,
    /// Each route's amount after its last hop, by index into the priority
    /// order. Only live routes' entries are meaningful.
    amounts: Vec<Amount>,
}

/// Clears the live routes' pools in `schedule` order, or `None` as soon as
/// one live route fails a leg.
fn run_pass(
    pools: &BTreeMap<PairId, Pool>,
    swaps: &BTreeMap<PairId, Vec<SwapIntent>>,
    order: &[&RouteIntent],
    live: &[bool],
    schedule: &[PairId],
) -> Option<Pass> {
    let mut amounts: Vec<Amount> = order.iter().map(|route| route.amount_in).collect();
    let mut next = alloc::vec![0_usize; order.len()];
    let mut cleared = Vec::with_capacity(schedule.len());

    for pair in schedule {
        let mut intents = swaps.get(pair).cloned().unwrap_or_default();
        let mut legs: Vec<usize> = Vec::new();
        for (index, route) in order.iter().enumerate() {
            let Some(hop) = route.hops.get(next[index]) else {
                continue;
            };
            if !live[index] || hop.pair != *pair {
                continue;
            }
            let last = next[index] + 1 == route.hops.len();
            intents.push(SwapIntent {
                id: route.id,
                trader: route.trader,
                direction: hop.direction,
                amount_in: amounts[index],
                // Intermediate legs only need a nonzero output, which the
                // batch already insists on; the bound is the route's last.
                min_out: if last { route.min_out } else { Amount::ZERO },
            });
            legs.push(index);
        }

        let outcome = clear_one(pools, pair, &intents);
        for &index in &legs {
            let paid = outcome.as_ref().and_then(|outcome| {
                outcome
                    .cleared
                    .iter()
                    .find(|trade| trade.id == order[index].id)
                    .map(|trade| trade.amount_out)
            })?;
            amounts[index] = paid;
            next[index] += 1;
        }
        cleared.push(PoolClearing {
            pair: *pair,
            outcome: outcome.map(|outcome| without_legs(outcome, order, &legs)),
        });
    }

    Some(Pass {
        pools: cleared,
        amounts,
    })
}

/// Removes route legs from a pool's outcome, so it lists plain swaps only.
fn without_legs(mut outcome: BatchOutcome, order: &[&RouteIntent], legs: &[usize]) -> BatchOutcome {
    let is_leg = |id: &[u8; 32]| legs.iter().any(|&index| order[index].id == *id);
    outcome.cleared.retain(|trade| !is_leg(&trade.id));
    outcome.skipped.retain(|id| !is_leg(id));
    outcome
}
