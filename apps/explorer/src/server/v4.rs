//! Routes for the v4 records (ADR-135 I8): `/name`, `/asset`, `/nft` and
//! `/schedule`, each a live read from the node. An owner as of the last
//! indexed block would be shown as current and quietly be wrong after a
//! transfer, so none of these come from the index.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};

use super::{html, html_status};
use crate::error::{ExplorerError, Result};
use crate::ui;

/// Query for the name lookup form.
#[derive(Debug, Deserialize)]
pub(super) struct NameQuery {
    label: Option<String>,
}

/// Query for the asset and schedule forms.
#[derive(Debug, Deserialize)]
pub(super) struct IdQuery {
    id: Option<String>,
}

/// Query for the NFT form.
#[derive(Debug, Deserialize)]
pub(super) struct NftQuery {
    collection: Option<String>,
    index: Option<String>,
}

/// A name's label as the chain stores it: lowercase, without `.maya`, and
/// passing the node's own label rule, so a lookup never reaches the node for
/// a key no transaction could have written.
fn name_label(input: &str) -> std::result::Result<String, String> {
    let lower = input.trim().to_ascii_lowercase();
    let label = lower.strip_suffix(".maya").unwrap_or(&lower).to_string();
    custom_l1_node::core::name_payload::validate_label(label.as_bytes())
        .map_err(|why| format!("Not a name the chain can hold: {why}."))?;
    Ok(label)
}

/// A 32-byte id as lowercase hex, or why not.
fn id_hex(input: &str, what: &str) -> std::result::Result<String, String> {
    let lower = input.trim().to_ascii_lowercase();
    let lower = lower.strip_prefix("0x").unwrap_or(&lower).to_string();
    if lower.len() == 64 && lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(lower)
    } else {
        Err(format!("Not {what}: an id is 64 hex characters."))
    }
}

/// How long one v4 read may hold a node connection. The pages are public and
/// unauthenticated; the client's 60 s default would let each one pin a slow
/// node for a minute.
const NODE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The one node client every v4 read shares, built with the router, or why
/// there is none.
#[derive(Clone)]
pub(super) struct V4Node(Arc<std::result::Result<HttpClient, String>>);

impl V4Node {
    pub(super) fn new(url: Option<&str>) -> Self {
        let client = match url {
            None => Err("No node endpoint configured.".to_string()),
            Some(url) => HttpClientBuilder::default()
                .request_timeout(NODE_READ_TIMEOUT)
                .build(url)
                .map_err(|e| format!("Node unavailable: {e}")),
        };
        Self(Arc::new(client))
    }
}

/// One read from the node: `(record, error)` as a page shows them.
async fn read<T: DeserializeOwned + Send>(
    node: &V4Node,
    method: &str,
    params: jsonrpsee::core::params::ArrayParams,
) -> (Option<T>, Option<String>) {
    use jsonrpsee::core::client::ClientT;

    let client = match node.0.as_ref() {
        Ok(client) => client,
        Err(why) => return (None, Some(why.clone())),
    };
    match client.request::<Option<T>, _>(method, params).await {
        Ok(record) => (record, None),
        Err(e) => (
            None,
            Some(format!(
                "Node unavailable or too old to serve {method}: {e}"
            )),
        ),
    }
}

/// `/name?label=`.
pub(super) async fn name_lookup(
    Extension(node): Extension<V4Node>,
    Query(query): Query<NameQuery>,
) -> Result<Response> {
    let input = query.label.unwrap_or_default();
    if input.trim().is_empty() {
        return Ok(html(ui::name_page(String::new(), None, None)));
    }
    let label = match name_label(&input) {
        Ok(label) => label,
        Err(message) => {
            return Ok(html_status(
                StatusCode::BAD_REQUEST,
                ui::name_page(input, None, Some(message)),
            ));
        }
    };
    let (info, error) = read(&node, "get_name", jsonrpsee::rpc_params![&label]).await;
    Ok(html(ui::name_page(format!("{label}.maya"), info, error)))
}

/// `/api/name/{label}`: the node's answer, status included.
pub(super) async fn api_name(
    Extension(node): Extension<V4Node>,
    Path(label): Path<String>,
) -> Result<Json<Option<custom_l1_node::rpc::NameInfo>>> {
    let label = name_label(&label).map_err(ExplorerError::BadRequest)?;
    match read::<custom_l1_node::rpc::NameInfo>(&node, "get_name", jsonrpsee::rpc_params![&label])
        .await
    {
        // As the gateway answers (docs/sdk.md): to a program a lapsed name is
        // free, so it reads as one never registered. The `/name` page still
        // shows the record, with its "does not resolve" banner.
        (info, None) => Ok(Json(info.filter(|i| i.status != "lapsed"))),
        (_, Some(error)) => Err(ExplorerError::NodeRpc(error)),
    }
}

/// `/asset?id=`.
pub(super) async fn asset_lookup(
    Extension(node): Extension<V4Node>,
    Query(query): Query<IdQuery>,
) -> Result<Response> {
    let input = query.id.unwrap_or_default();
    if input.trim().is_empty() {
        return Ok(html(ui::asset_page(String::new(), None, None)));
    }
    match id_hex(&input, "an asset id") {
        Err(message) => Ok(html_status(
            StatusCode::BAD_REQUEST,
            ui::asset_page(input, None, Some(message)),
        )),
        Ok(id) => {
            let (info, error) = read(&node, "get_asset", jsonrpsee::rpc_params![&id]).await;
            Ok(html(ui::asset_page(id, info, error)))
        }
    }
}

/// `/schedule?id=`.
pub(super) async fn schedule_lookup(
    Extension(node): Extension<V4Node>,
    Query(query): Query<IdQuery>,
) -> Result<Response> {
    let input = query.id.unwrap_or_default();
    if input.trim().is_empty() {
        return Ok(html(ui::schedule_page(String::new(), None, None)));
    }
    match id_hex(&input, "a schedule id") {
        Err(message) => Ok(html_status(
            StatusCode::BAD_REQUEST,
            ui::schedule_page(input, None, Some(message)),
        )),
        Ok(id) => {
            let (info, error) = read(&node, "get_schedule", jsonrpsee::rpc_params![&id]).await;
            Ok(html(ui::schedule_page(id, info, error)))
        }
    }
}

/// `/nft?collection=&index=`: the collection, and one item when an index is given.
pub(super) async fn nft_lookup(
    Extension(node): Extension<V4Node>,
    Query(query): Query<NftQuery>,
) -> Result<Response> {
    let input = query.collection.unwrap_or_default();
    let index_text = query.index.unwrap_or_default().trim().to_string();
    if input.trim().is_empty() {
        return Ok(html(ui::nft_page((String::new(), index_text), None, None)));
    }
    let index = match index_text.as_str() {
        "" => Ok(None),
        text => text
            .parse::<u64>()
            .map(Some)
            .map_err(|_| "Not an item index: a whole number.".to_string()),
    };
    let (id, index) = match (id_hex(&input, "a collection id"), index) {
        (Ok(id), Ok(index)) => (id, index),
        (Err(message), _) | (_, Err(message)) => {
            return Ok(html_status(
                StatusCode::BAD_REQUEST,
                ui::nft_page((input, index_text), None, Some(message)),
            ));
        }
    };
    let (collection, error) = read(&node, "get_nft_collection", jsonrpsee::rpc_params![&id]).await;
    let (item, item_error) = match (&collection, index) {
        (Some(_), Some(i)) => read(&node, "get_nft_item", jsonrpsee::rpc_params![&id, i]).await,
        _ => (None, None),
    };
    let found = collection.map(|c| (c, item));
    Ok(html(ui::nft_page(
        (id, index_text),
        found,
        error.or(item_error),
    )))
}
