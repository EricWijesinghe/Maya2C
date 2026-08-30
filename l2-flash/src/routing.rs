//! Multi-hop routing across the channel network.
//!
//! ## On the graph's shape
//!
//! A payment-channel network is **not** a DAG. It is an undirected multigraph,
//! densely cyclic — any two nodes may hold several channels, and cycles are the
//! norm rather than the exception.
//!
//! What *is* acyclic is any individual route. This module models each channel as
//! **two directed edges with independent capacity** — what A can push to B is
//! not what B can push to A — and searches with no node revisited, so every
//! route it returns is a simple, acyclic path. That is the accurate description;
//! calling the search itself a "DAG pathfinder" would misdescribe both the input
//! and the algorithm.
//!
//! ## Algorithm
//!
//! Dijkstra over the directed capacity graph, minimising total fee. Edges whose
//! directional capacity cannot carry the amount are skipped rather than
//! penalised: a channel that cannot forward the payment is not a worse route, it
//! is not a route at all.
//!
//! Fees accumulate *backwards* — an intermediary forwards less than it receives,
//! keeping the difference — so the search runs from the destination outward and
//! grows the amount as it goes. Searching forward would require knowing the
//! final amount before the fees that determine it.

use std::collections::{BinaryHeap, HashMap, HashSet};

use custom_l1_node::core::ChannelId;

use crate::error::{FlashError, Result};

/// A node in the channel network.
pub type NodeId = [u8; 32];

/// Default ceiling on hops.
///
/// Long routes lock value at every intermediary and multiply the chance any one
/// of them stalls, so an unbounded search is not merely slow but operationally
/// worse.
pub const MAX_HOPS: usize = 8;

/// What a node charges to forward a payment.
///
/// Grouped rather than passed as two loose integers: a base fee and a
/// proportional rate are one policy, and adjacent `u64` parameters are easy to
/// transpose silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FeePolicy {
    /// Flat fee charged per forward.
    pub base_fee: u64,
    /// Additional fee in parts-per-million of the forwarded amount.
    pub fee_ppm: u64,
}

impl FeePolicy {
    /// A policy charging `base_fee` plus `fee_ppm` parts per million.
    #[must_use]
    pub const fn new(base_fee: u64, fee_ppm: u64) -> Self {
        Self { base_fee, fee_ppm }
    }

    /// A policy that charges nothing.
    pub const FREE: Self = Self {
        base_fee: 0,
        fee_ppm: 0,
    };
}

/// One directed channel edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// Channel backing this edge.
    pub channel_id: ChannelId,
    /// Node that can send.
    pub from: NodeId,
    /// Node that receives.
    pub to: NodeId,
    /// Value `from` can currently push toward `to`.
    pub capacity: u64,
    /// Flat fee charged for forwarding.
    pub base_fee: u64,
    /// Fee in parts-per-million of the forwarded amount.
    pub fee_ppm: u64,
}

impl Edge {
    /// Fee this edge charges to forward `amount`.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::BalanceOverflow`] on overflow.
    pub fn fee_for(&self, amount: u64) -> Result<u64> {
        let proportional = (u128::from(amount) * u128::from(self.fee_ppm)) / 1_000_000;
        let proportional = u64::try_from(proportional).map_err(|_| FlashError::BalanceOverflow)?;
        self.base_fee
            .checked_add(proportional)
            .ok_or(FlashError::BalanceOverflow)
    }
}

/// One hop of a computed route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    /// Channel used.
    pub channel_id: ChannelId,
    /// Sending node.
    pub from: NodeId,
    /// Receiving node.
    pub to: NodeId,
    /// Value entering this hop, inclusive of downstream fees.
    pub amount_in: u64,
    /// Fee retained by `from` for forwarding.
    pub fee: u64,
}

/// A complete path from sender to recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// Hops in send order.
    pub hops: Vec<Hop>,
    /// Amount the sender must commit, fees included.
    pub total_amount: u64,
    /// Amount the recipient receives.
    pub delivered: u64,
}

impl Route {
    /// Total fees paid across the route.
    #[must_use]
    pub fn total_fees(&self) -> u64 {
        self.total_amount.saturating_sub(self.delivered)
    }

    /// Number of hops.
    #[must_use]
    pub fn hop_count(&self) -> usize {
        self.hops.len()
    }
}

/// The routable view of the network.
#[derive(Clone, Debug, Default)]
pub struct ChannelGraph {
    /// Outgoing edges by sender.
    outgoing: HashMap<NodeId, Vec<Edge>>,
    /// Incoming edges by receiver, for the reverse search.
    incoming: HashMap<NodeId, Vec<Edge>>,
}

impl ChannelGraph {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a channel as two independently-capacitated directed edges.
    pub fn add_channel(
        &mut self,
        channel_id: ChannelId,
        node_a: NodeId,
        node_b: NodeId,
        capacity_a_to_b: u64,
        capacity_b_to_a: u64,
        fees: FeePolicy,
    ) {
        for (from, to, capacity) in [
            (node_a, node_b, capacity_a_to_b),
            (node_b, node_a, capacity_b_to_a),
        ] {
            let edge = Edge {
                channel_id,
                from,
                to,
                capacity,
                base_fee: fees.base_fee,
                fee_ppm: fees.fee_ppm,
            };
            self.outgoing.entry(from).or_default().push(edge.clone());
            self.incoming.entry(to).or_default().push(edge);
        }
    }

    /// Number of directed edges.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.outgoing.values().map(Vec::len).sum()
    }

    /// Number of nodes with at least one outgoing edge.
    #[must_use]
    pub fn node_count(&self) -> usize {
        let mut nodes: HashSet<NodeId> = self.outgoing.keys().copied().collect();
        nodes.extend(self.incoming.keys().copied());
        nodes.len()
    }

    /// Edges leaving `node`.
    #[must_use]
    pub fn edges_from(&self, node: &NodeId) -> &[Edge] {
        self.outgoing.get(node).map_or(&[], Vec::as_slice)
    }

    /// Updates one direction's capacity after a payment moves value.
    ///
    /// Returns `true` if the edge was found.
    pub fn set_capacity(&mut self, channel_id: &ChannelId, from: &NodeId, capacity: u64) -> bool {
        let mut found = false;
        if let Some(edges) = self.outgoing.get_mut(from) {
            for edge in edges.iter_mut() {
                if &edge.channel_id == channel_id {
                    edge.capacity = capacity;
                    found = true;
                }
            }
        }
        for edges in self.incoming.values_mut() {
            for edge in edges.iter_mut() {
                if &edge.channel_id == channel_id && &edge.from == from {
                    edge.capacity = capacity;
                }
            }
        }
        found
    }

    /// Finds the cheapest route delivering `amount` from `source` to `target`.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::NoRoute`] when no path has the capacity, or
    /// [`FlashError::RouteTooLong`] if the cheapest path exceeds `max_hops`.
    pub fn find_route(
        &self,
        source: &NodeId,
        target: &NodeId,
        amount: u64,
        max_hops: usize,
    ) -> Result<Route> {
        if source == target {
            return Err(FlashError::NoRoute {
                from: hex::encode(source),
                to: hex::encode(target),
                amount,
            });
        }

        // Searching backwards from the target: the amount a hop must carry
        // depends on the fees of everything downstream of it, which are only
        // known once that suffix is fixed.
        let best = self.search_backwards(source, target, amount, max_hops)?;
        self.build_route(source, target, amount, &best, max_hops)
    }

    /// Dijkstra from `target` along reversed edges.
    ///
    /// Returns, per node, the amount that node must send for the payment to
    /// deliver `amount`, together with the next hop toward the target.
    fn search_backwards(
        &self,
        source: &NodeId,
        target: &NodeId,
        amount: u64,
        max_hops: usize,
    ) -> Result<HashMap<NodeId, (u64, Edge)>> {
        // Ordered by amount-to-send: minimising what the sender pays is exactly
        // minimising accumulated fees.
        let mut best: HashMap<NodeId, (u64, Edge)> = HashMap::new();
        let mut settled: HashSet<NodeId> = HashSet::new();
        let mut queue: BinaryHeap<Candidate> = BinaryHeap::new();

        queue.push(Candidate {
            cost: amount,
            hops: 0,
            node: *target,
        });

        while let Some(Candidate { cost, hops, node }) = queue.pop() {
            if !settled.insert(node) {
                continue;
            }
            if &node == source {
                break;
            }
            if hops >= max_hops {
                continue;
            }

            for edge in self.incoming.get(&node).map_or(&[][..], Vec::as_slice) {
                // A channel that cannot carry the amount is not a route at all.
                if edge.capacity < cost {
                    continue;
                }
                // No revisiting: this is what keeps every route acyclic.
                if settled.contains(&edge.from) {
                    continue;
                }

                let fee = if &edge.from == source {
                    // The sender pays no forwarding fee to itself.
                    0
                } else {
                    edge.fee_for(cost)?
                };
                let Some(upstream_cost) = cost.checked_add(fee) else {
                    continue;
                };

                let improves = best
                    .get(&edge.from)
                    .is_none_or(|(existing, _)| upstream_cost < *existing);
                if improves {
                    best.insert(edge.from, (upstream_cost, edge.clone()));
                    queue.push(Candidate {
                        cost: upstream_cost,
                        hops: hops + 1,
                        node: edge.from,
                    });
                }
            }
        }

        if !best.contains_key(source) {
            return Err(FlashError::NoRoute {
                from: hex::encode(source),
                to: hex::encode(target),
                amount,
            });
        }

        Ok(best)
    }

    /// Walks the search result from source to target, materialising hops.
    fn build_route(
        &self,
        source: &NodeId,
        target: &NodeId,
        amount: u64,
        best: &HashMap<NodeId, (u64, Edge)>,
        max_hops: usize,
    ) -> Result<Route> {
        let mut hops = Vec::new();
        let mut cursor = *source;
        let mut carried =
            best.get(source)
                .map(|(cost, _)| *cost)
                .ok_or_else(|| FlashError::NoRoute {
                    from: hex::encode(source),
                    to: hex::encode(target),
                    amount,
                })?;
        let total_amount = carried;

        while &cursor != target {
            let (_, edge) = best.get(&cursor).ok_or_else(|| FlashError::NoRoute {
                from: hex::encode(source),
                to: hex::encode(target),
                amount,
            })?;

            let fee = if &edge.from == source {
                0
            } else {
                // What this node keeps is the difference between what it
                // receives and what it must forward.
                let downstream = best.get(&edge.to).map_or(amount, |(cost, _)| *cost);
                carried.saturating_sub(downstream)
            };

            hops.push(Hop {
                channel_id: edge.channel_id,
                from: edge.from,
                to: edge.to,
                amount_in: carried,
                fee,
            });

            carried = carried.saturating_sub(fee);
            cursor = edge.to;

            if hops.len() > max_hops {
                return Err(FlashError::RouteTooLong {
                    hops: hops.len(),
                    max: max_hops,
                });
            }
        }

        Ok(Route {
            hops,
            total_amount,
            delivered: amount,
        })
    }
}

/// Dijkstra priority-queue entry.
///
/// `Ord` is deliberately reversed so `BinaryHeap` — a max-heap — pops the
/// cheapest candidate.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Candidate {
    cost: u64,
    hops: usize,
    node: NodeId,
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        other
            .cost
            .cmp(&self.cost)
            .then_with(|| other.hops.cmp(&self.hops))
            // Tie-break on node id so ordering is total and the search is
            // deterministic across runs.
            .then_with(|| self.node.cmp(&other.node))
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
