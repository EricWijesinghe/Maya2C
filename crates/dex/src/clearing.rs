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
//! be honoured while each pool clears once, so routes are admitted in
//! identifier order and one whose edges would close a cycle is a **no-op**: its
//! input is returned and nothing moves (invariant 7). A route that visits one
//! pool twice closes a cycle with itself; the node refuses it at staging,
//! because it could never execute.
//!
//! # All or nothing, across pools
//!
//! A route is atomic: either every leg settles and its final output meets
//! `min_out`, or the route is refunded whole. The clearing therefore runs to a
//! fixed point. Each pass clears every route-carrying pool in order; a route
//! that missed its bound, or whose leg a pool could not price, is removed, and
//! the pass is recomputed from the original pools without it. Every pass but
//! the last removes at least one route, so there are at most
//! `routes + 1` passes, each bounded by [`MAX_BLOCK_ROUTES`] and
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
/// of `routes` or of the intents within `swaps`.
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
    order.sort_by_key(|route| route.id);

    let (graph, admitted) = admit(pools, &order);
    let route_pools: BTreeSet<PairId> = order
        .iter()
        .zip(&admitted)
        .filter(|(_, admitted)| **admitted)
        .flat_map(|(route, _)| route.hops.iter().map(|hop| hop.pair))
        .collect();
    let schedule = topological(&graph, &route_pools);

    let mut live = admitted;
    let (mut cleared, amounts) = loop {
        let pass = run_pass(pools, swaps, &order, &live, &schedule);
        if pass.failed.is_empty() {
            break (pass.pools, pass.amounts);
        }
        for index in pass.failed {
            live[index] = false;
        }
    };

    // Pools no route touches, once. Their order among themselves is pair order.
    for (pair, intents) in swaps {
        if !route_pools.contains(pair) && !intents.is_empty() {
            cleared.push(PoolClearing {
                pair: *pair,
                outcome: clear_one(pools, pair, intents),
            });
        }
    }

    let routes = order
        .iter()
        .enumerate()
        .map(|(index, route)| RouteSettlement {
            id: route.id,
            trader: route.trader,
            amount_out: if live[index] {
                Some(amounts[index])
            } else {
                None
            },
        })
        .collect();
    Ok(BlockClearing {
        pools: cleared,
        routes,
    })
}

/// Admits routes in identifier order while the precedence graph stays acyclic.
fn admit(
    pools: &BTreeMap<PairId, Pool>,
    order: &[&RouteIntent],
) -> (BTreeMap<PairId, BTreeSet<PairId>>, Vec<bool>) {
    let mut graph: BTreeMap<PairId, BTreeSet<PairId>> = BTreeMap::new();
    let mut admitted = Vec::with_capacity(order.len());
    for route in order {
        let distinct: BTreeSet<PairId> = route.hops.iter().map(|hop| hop.pair).collect();
        let known = route.hops.iter().all(|hop| pools.contains_key(&hop.pair));
        if distinct.len() != route.hops.len() || !known {
            admitted.push(false);
            continue;
        }
        let mut candidate = graph.clone();
        for window in route.hops.windows(2) {
            candidate
                .entry(window[0].pair)
                .or_default()
                .insert(window[1].pair);
        }
        let nodes = distinct_nodes(&candidate);
        let acyclic = topological(&candidate, &nodes).len() == nodes.len();
        if acyclic {
            graph = candidate;
        }
        admitted.push(acyclic);
    }
    (graph, admitted)
}

/// Every node the graph names, as a source or a target.
fn distinct_nodes(graph: &BTreeMap<PairId, BTreeSet<PairId>>) -> BTreeSet<PairId> {
    graph
        .iter()
        .flat_map(|(from, to)| core::iter::once(*from).chain(to.iter().copied()))
        .collect()
}

/// Kahn's algorithm over `nodes`, smallest ready identifier first. A cycle
/// leaves its nodes out, which is how [`admit`] detects one.
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

/// One pass over the route-carrying pools.
struct Pass {
    pools: Vec<PoolClearing>,
    /// Each route's amount after the last hop it completed.
    amounts: Vec<Amount>,
    /// Routes that failed this pass, by index into the identifier order.
    failed: Vec<usize>,
}

fn run_pass(
    pools: &BTreeMap<PairId, Pool>,
    swaps: &BTreeMap<PairId, Vec<SwapIntent>>,
    order: &[&RouteIntent],
    live: &[bool],
    schedule: &[PairId],
) -> Pass {
    let mut amounts: Vec<Amount> = order.iter().map(|route| route.amount_in).collect();
    let mut next = alloc::vec![0_usize; order.len()];
    let mut alive: Vec<bool> = live.to_vec();
    let mut failed = Vec::new();
    let mut cleared = Vec::with_capacity(schedule.len());

    for pair in schedule {
        let mut intents = swaps.get(pair).cloned().unwrap_or_default();
        let mut legs: Vec<usize> = Vec::new();
        for (index, route) in order.iter().enumerate() {
            let Some(hop) = route.hops.get(next[index]) else {
                continue;
            };
            if !alive[index] || hop.pair != *pair {
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
            });
            if let Some(amount) = paid {
                amounts[index] = amount;
                next[index] += 1;
            } else {
                alive[index] = false;
                failed.push(index);
            }
        }
        cleared.push(PoolClearing {
            pair: *pair,
            outcome: outcome.map(|outcome| without_legs(outcome, order, &legs)),
        });
    }

    Pass {
        pools: cleared,
        amounts,
        failed,
    }
}

/// Removes route legs from a pool's outcome, so it lists plain swaps only.
fn without_legs(mut outcome: BatchOutcome, order: &[&RouteIntent], legs: &[usize]) -> BatchOutcome {
    let is_leg = |id: &[u8; 32]| legs.iter().any(|&index| order[index].id == *id);
    outcome.cleared.retain(|trade| !is_leg(&trade.id));
    outcome.skipped.retain(|id| !is_leg(id));
    outcome
}
