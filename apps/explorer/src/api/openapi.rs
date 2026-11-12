//! The v1 OpenAPI document, generated from the handlers' annotations.
//!
//! `apps/explorer/openapi/v1.yaml` is the golden copy. A test regenerates the
//! document and fails on any difference, so a change to a route or a body is
//! a visible change to the file. The file holds JSON, which YAML 1.2 accepts;
//! see ADR-092 for why no YAML serializer is used.

use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use crate::api::chain;
use crate::api::dto::{
    Account, Block, Counts, Mempool, Output, Status, Supply, Transaction, TransactionDetail,
};
use crate::api::history;
use crate::api::problem::Code;

/// The RFC 9457 body every failure carries.
#[derive(Serialize, ToSchema)]
pub struct Problem {
    /// `urn:maya2c:error:<code>`.
    #[serde(rename = "type")]
    pub problem_type: String,
    /// The HTTP reason phrase.
    pub title: String,
    /// The HTTP status, repeated in the body.
    pub status: u16,
    /// What went wrong, for a person to read.
    pub detail: String,
    /// The stable failure code. Branch on this, not on `detail`.
    pub code: Code,
}

/// The whole v1 surface.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Maya2C indexer API",
        version = "1",
        description = "Read-only access to indexed history and live node values. \
            Amounts are decimal strings (256-bit). Hashes are lowercase hex. \
            Addresses are Bech32m. Timestamps are unix seconds. Lists are \
            paged by opaque cursor; a null next_cursor ends the walk."
    ),
    paths(
        chain::status,
        chain::account,
        chain::validators,
        chain::supply,
        chain::mempool,
        history::blocks,
        history::block,
        history::block_transactions,
        history::transactions,
        history::transaction,
        history::account_transactions,
    ),
    components(schemas(
        Problem,
        Code,
        Account,
        Block,
        Counts,
        Mempool,
        Output,
        Status,
        Supply,
        Transaction,
        TransactionDetail,
    )),
    tags(
        (name = "chain", description = "Chain status, validators, supply and mempool"),
        (name = "blocks", description = "Indexed blocks"),
        (name = "transactions", description = "Indexed transactions"),
        (name = "accounts", description = "Accounts: live balance and address history"),
    )
)]
pub struct ApiDoc;

/// The v1 document as generated now.
#[must_use]
pub fn document() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
