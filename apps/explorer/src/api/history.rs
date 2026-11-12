//! History routes: blocks, transactions and an address's transactions.
//!
//! All of it is answered from the index, so none of it needs the node. The
//! chain's network and finality are read from the node's cache, and a node
//! that is down only blanks the fields that need it.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use maya_address::Network;
use serde::Deserialize;

use crate::api::address;
use crate::api::chain::index_error;
use crate::api::dto::{Block, Output, Page, Transaction, TransactionDetail};
use crate::api::problem::{ApiError, Code};
use crate::api::query::{self, BlockRef};
use crate::api::respond::{self, Freshness, Meta};
use crate::api::{ApiState, chain_meta, finalized_height, network_of};
use crate::model::{IndexedBlock, IndexedTx};
use crate::store::Seek;

/// Query string for every list route.
#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    /// Page size, 1 to 100. Default 25.
    pub limit: Option<String>,
    /// Cursor from `next_cursor` of the previous page.
    pub cursor: Option<String>,
    /// `sent`, `received` or `both` (address routes only). Default `both`.
    pub direction: Option<String>,
}

/// Meta every history response carries: the index and the node's tip.
async fn meta_for(state: &ApiState, indexed: Option<i64>) -> Meta {
    Meta {
        indexed,
        tip: chain_meta(state).await.map(|info| info.height),
        cache: None,
    }
}

async fn indexed_height(state: &ApiState) -> Result<Option<i64>, ApiError> {
    state
        .store
        .latest_height()
        .await
        .map_err(|e| index_error(&e))
}

/// Resolves a block reference, or `404 not_found`.
async fn find_block(state: &ApiState, reference: &BlockRef) -> Result<IndexedBlock, ApiError> {
    let found = match reference {
        BlockRef::Height(height) => state.store.block_by_height(*height).await,
        BlockRef::Id(id) => state.store.block_by_id(id).await,
    }
    .map_err(|e| index_error(&e))?;
    found.ok_or_else(|| ApiError::new(Code::NotFound, "no indexed block at that height or id"))
}

/// Newest blocks first, by height.
#[utoipa::path(
    get,
    path = "/api/v1/blocks",
    tag = "blocks",
    params(
        ("limit" = Option<String>, Query, description = "1 to 100, default 25"),
        ("cursor" = Option<String>, Query, description = "Opaque cursor from next_cursor"),
    ),
    responses(
        (status = 200, description = "A page of blocks.", body = Page<Block>),
        (status = 400, description = "invalid_param or invalid_cursor", body = crate::api::openapi::Problem),
    )
)]
pub async fn blocks(
    State(state): State<ApiState>,
    Query(params): Query<ListParams>,
) -> Result<Response, ApiError> {
    let limit = query::limit(params.limit.as_deref())?;
    let before = query::cursor(params.cursor.as_deref())?;
    let rows = state
        .store
        .blocks_page(before, limit)
        .await
        .map_err(|e| index_error(&e))?;
    let keyed = rows
        .iter()
        .map(|block| {
            let seek = Seek {
                height: block.height,
                position: 0,
            };
            (seek, Block::from(block))
        })
        .collect();
    let page = Page::new(keyed, limit, |seek| *seek);
    let meta = meta_for(&state, indexed_height(&state).await?).await;
    Ok(respond::json(&page, Freshness::Short, meta))
}

/// One block, by height or by id.
#[utoipa::path(
    get,
    path = "/api/v1/blocks/{reference}",
    tag = "blocks",
    params(("reference" = String, Path, description = "Height, or 64-hex block id")),
    responses(
        (status = 200, description = "The block.", body = Block),
        (status = 400, description = "invalid_param", body = crate::api::openapi::Problem),
        (status = 404, description = "not_found", body = crate::api::openapi::Problem),
    )
)]
pub async fn block(
    State(state): State<ApiState>,
    Path(reference): Path<String>,
) -> Result<Response, ApiError> {
    let reference = query::block_ref(&reference)?;
    let found = find_block(&state, &reference).await?;
    let freshness = respond::for_height(found.height, finalized_height(&state).await);
    let meta = meta_for(&state, indexed_height(&state).await?).await;
    Ok(respond::json(&Block::from(&found), freshness, meta))
}

/// Transactions in one block, in inclusion order.
///
/// Ascending, unlike the other lists: the cursor continues after the position
/// it names.
#[utoipa::path(
    get,
    path = "/api/v1/blocks/{reference}/transactions",
    tag = "blocks",
    params(
        ("reference" = String, Path, description = "Height, or 64-hex block id"),
        ("limit" = Option<String>, Query, description = "1 to 100, default 25"),
        ("cursor" = Option<String>, Query, description = "Opaque cursor from next_cursor"),
    ),
    responses(
        (status = 200, description = "A page of the block's transactions.", body = Page<Transaction>),
        (status = 400, description = "invalid_param or invalid_cursor", body = crate::api::openapi::Problem),
        (status = 404, description = "not_found", body = crate::api::openapi::Problem),
    )
)]
pub async fn block_transactions(
    State(state): State<ApiState>,
    Path(reference): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Response, ApiError> {
    let reference = query::block_ref(&reference)?;
    let limit = query::limit(params.limit.as_deref())?;
    let after = query::cursor(params.cursor.as_deref())?;
    let block = find_block(&state, &reference).await?;
    if after.is_some_and(|seek| seek.height != block.height) {
        return Err(ApiError::new(
            Code::InvalidCursor,
            "cursor belongs to a different block",
        ));
    }
    let network = network_of(&state).await;
    let txs = state
        .store
        .transactions_in_block(block.height)
        .await
        .map_err(|e| index_error(&e))?;
    let keyed = placed(block.height, &txs, network)
        .into_iter()
        .filter(|(seek, _)| after.is_none_or(|a| seek.position > a.position))
        .take(usize::try_from(limit).unwrap_or(0))
        .collect();
    let page = Page::new(keyed, limit, |seek| *seek);
    let freshness = respond::for_height(block.height, finalized_height(&state).await);
    let meta = meta_for(&state, indexed_height(&state).await?).await;
    Ok(respond::json(&page, freshness, meta))
}

/// A block's transactions with their positions, as v1 rows.
fn placed(height: i64, txs: &[IndexedTx], network: Option<Network>) -> Vec<(Seek, Transaction)> {
    txs.iter()
        .enumerate()
        .map(|(position, tx)| {
            let seek = Seek {
                height,
                position: i64::try_from(position).unwrap_or(i64::MAX),
            };
            (seek, Transaction::new(seek, tx, network))
        })
        .collect()
}

/// Transactions across the chain, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/transactions",
    tag = "transactions",
    params(
        ("limit" = Option<String>, Query, description = "1 to 100, default 25"),
        ("cursor" = Option<String>, Query, description = "Opaque cursor from next_cursor"),
    ),
    responses(
        (status = 200, description = "A page of transactions.", body = Page<Transaction>),
        (status = 400, description = "invalid_param or invalid_cursor", body = crate::api::openapi::Problem),
    )
)]
pub async fn transactions(
    State(state): State<ApiState>,
    Query(params): Query<ListParams>,
) -> Result<Response, ApiError> {
    let limit = query::limit(params.limit.as_deref())?;
    let before = query::cursor(params.cursor.as_deref())?;
    let network = network_of(&state).await;
    let rows = state
        .store
        .transactions_page(before, limit)
        .await
        .map_err(|e| index_error(&e))?;
    let keyed = rows
        .iter()
        .map(|(seek, tx)| (*seek, Transaction::new(*seek, tx, network)))
        .collect();
    let page = Page::new(keyed, limit, |seek| *seek);
    let meta = meta_for(&state, indexed_height(&state).await?).await;
    Ok(respond::json(&page, Freshness::Short, meta))
}

/// One transaction with its outputs.
#[utoipa::path(
    get,
    path = "/api/v1/transactions/{txid}",
    tag = "transactions",
    params(("txid" = String, Path, description = "64-hex transaction id")),
    responses(
        (status = 200, description = "The transaction and its outputs.", body = TransactionDetail),
        (status = 400, description = "invalid_param", body = crate::api::openapi::Problem),
        (status = 404, description = "not_found", body = crate::api::openapi::Problem),
    )
)]
pub async fn transaction(
    State(state): State<ApiState>,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    let txid = query::txid(&raw)?;
    let tx = state
        .store
        .transaction(&txid)
        .await
        .map_err(|e| index_error(&e))?
        .ok_or_else(|| ApiError::new(Code::NotFound, "no indexed transaction with that id"))?;
    let block_txs = state
        .store
        .transactions_in_block(tx.height)
        .await
        .map_err(|e| index_error(&e))?;
    let position = block_txs
        .iter()
        .position(|other| other.txid == txid)
        .unwrap_or(0);
    let seek = Seek {
        height: tx.height,
        position: i64::try_from(position).unwrap_or(i64::MAX),
    };
    let network = network_of(&state).await;
    let outputs = state
        .store
        .outputs_of(&txid)
        .await
        .map_err(|e| index_error(&e))?;
    let body = TransactionDetail {
        transaction: Transaction::new(seek, &tx, network),
        outputs: outputs
            .iter()
            .map(|output| Output::new(output, network))
            .collect(),
    };
    let freshness = respond::for_height(tx.height, finalized_height(&state).await);
    let meta = meta_for(&state, indexed_height(&state).await?).await;
    Ok(respond::json(&body, freshness, meta))
}

/// Transactions an address sent, received or both, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{address}/transactions",
    tag = "accounts",
    params(
        ("address" = String, Path, description = "Bech32m address (ADR-081)"),
        ("direction" = Option<String>, Query, description = "sent, received or both (default)"),
        ("limit" = Option<String>, Query, description = "1 to 100, default 25"),
        ("cursor" = Option<String>, Query, description = "Opaque cursor from next_cursor"),
    ),
    responses(
        (status = 200, description = "A page of the address's transactions.", body = Page<Transaction>),
        (status = 400, description = "invalid_address, wrong_network, invalid_param or invalid_cursor", body = crate::api::openapi::Problem),
    )
)]
pub async fn account_transactions(
    State(state): State<ApiState>,
    Path(raw): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Response, ApiError> {
    let network = network_of(&state).await;
    let id = address::parse(&raw, network)?;
    let limit = query::limit(params.limit.as_deref())?;
    let direction = query::direction(params.direction.as_deref())?;
    let before = query::cursor(params.cursor.as_deref())?;
    let rows = state
        .store
        .transactions_by_address(&hex::encode(id), direction, before, limit)
        .await
        .map_err(|e| index_error(&e))?;
    let keyed = rows
        .iter()
        .map(|(seek, tx)| (*seek, Transaction::new(*seek, tx, network)))
        .collect();
    let page = Page::new(keyed, limit, |seek| *seek);
    let meta = meta_for(&state, indexed_height(&state).await?).await;
    Ok(respond::json(&page, Freshness::Short, meta))
}
