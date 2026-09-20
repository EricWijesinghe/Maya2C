//! GraphQL over the same node methods the REST layer serves.
//!
//! # Why the limits are not optional
//!
//! GraphQL lets a client compose its own query. Over a chain backed by RocksDB
//! that is an unbounded-work surface by construction: a nested query is a
//! request that the server, not the client, pays for, and a public endpoint
//! with no ceiling is a denial-of-service primitive that ships enabled.
//!
//! So [`schema`] installs a depth limit and a complexity limit at construction,
//! and there is no constructor without them. That is the same discipline the
//! rest of this tree applies to every length it reads off a wire — the ceiling
//! is checked before the work is done, not after it hurts.
//!
//! # What is queryable
//!
//! Exactly what the REST layer exposes, because both go through
//! [`NodeClient`] and the allowlist behind it. GraphQL is a second shape over
//! one surface, not a second surface.
//!
//! Notably absent: anything about the "DAG". `crypto::dag` in the node is the
//! memory-hard proof-of-work *dataset*, not a block graph, and it is not chain
//! state. The block-graph work in `maya-blockgraph` is a research branch that
//! no consensus path reaches. There is no DAG state to query, and inventing a
//! field that returned something plausible would be worse than its absence.

use std::sync::Arc;

use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};

use crate::node::NodeClient;

/// Maximum nesting depth of a query.
///
/// Six is comfortably past anything this schema needs — its deepest legitimate
/// path is three — and far short of what an adversary would use. Set from the
/// schema's own shape rather than copied from a tutorial.
pub const MAX_DEPTH: usize = 6;

/// Maximum complexity, in resolver-nodes, of a query.
///
/// Each field costs one by default. 200 admits a generous batch of account
/// lookups in one request while bounding the number of upstream calls a single
/// HTTP request can provoke — which is the quantity that actually matters,
/// because each one is a round trip to the node.
pub const MAX_COMPLEXITY: usize = 200;

/// An account's balance and nonce.
#[derive(SimpleObject)]
pub struct Account {
    /// Hex-encoded address, echoed back.
    pub address: String,
    /// Spendable amount in base units.
    pub balance: u64,
    /// Next valid nonce.
    pub nonce: u64,
}

/// Circulating and total supply.
#[derive(SimpleObject)]
pub struct Supply {
    /// Units in circulation.
    pub circulating: u64,
    /// Units issued in total.
    pub total: u64,
}

/// Root query.
pub struct Query;

#[Object]
impl Query {
    /// Balance and nonce for one account.
    async fn account(&self, ctx: &Context<'_>, address: String) -> async_graphql::Result<Account> {
        let node = ctx.data::<Arc<dyn NodeClient>>()?;
        let balance = node.get_balance(&address).await?;
        Ok(Account {
            address,
            balance: balance.balance,
            nonce: balance.nonce,
        })
    }

    /// Circulating and total supply.
    async fn supply(&self, ctx: &Context<'_>) -> async_graphql::Result<Supply> {
        let node = ctx.data::<Arc<dyn NodeClient>>()?;
        let supply = node.get_supply().await?;
        Ok(Supply {
            circulating: supply.circulating,
            total: supply.total,
        })
    }

    /// A block by height, as the node's own JSON.
    ///
    /// Returned as `Json` rather than modelled field by field. A block's shape
    /// is consensus and changes with the chain; a hand-maintained GraphQL
    /// mirror of it would drift silently, and a drifted mirror is worse than a
    /// passthrough because it looks authoritative.
    async fn block(
        &self,
        ctx: &Context<'_>,
        height: u64,
    ) -> async_graphql::Result<Option<async_graphql::Json<serde_json::Value>>> {
        let node = ctx.data::<Arc<dyn NodeClient>>()?;
        let block = node.get_block_by_height(height).await?;
        if block.is_null() {
            return Ok(None);
        }
        Ok(Some(async_graphql::Json(block)))
    }
}

/// The schema type this gateway serves.
pub type GatewaySchema = Schema<Query, EmptyMutation, EmptySubscription>;

/// Builds the schema with its limits installed.
///
/// There is deliberately no variant without limits. A `schema_unlimited` for
/// tests would be the version somebody eventually wired into `main`.
#[must_use]
pub fn schema(node: Arc<dyn NodeClient>) -> GatewaySchema {
    Schema::build(Query, EmptyMutation, EmptySubscription)
        .data(node)
        .limit_depth(MAX_DEPTH)
        .limit_complexity(MAX_COMPLEXITY)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_are_set_from_the_schema_shape() {
        // Pinned so a later "let's raise this" is a deliberate edit with a
        // failing test attached, not a quiet change to a DoS ceiling.
        assert_eq!(MAX_DEPTH, 6);
        assert_eq!(MAX_COMPLEXITY, 200);
    }

    #[test]
    fn mutations_are_not_exposed_over_graphql() {
        // Writes go through REST, where the size ceilings and the sealed
        // validation live. A GraphQL mutation would be a second write path
        // that has to re-implement both, and would drift from them.
        let type_name = std::any::type_name::<GatewaySchema>();
        assert!(
            type_name.contains("EmptyMutation"),
            "the schema must expose no mutations: {type_name}"
        );
    }
}
