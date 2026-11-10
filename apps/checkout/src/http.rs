//! The HTTP API.
//!
//! - `POST /v1/invoices`: creates an invoice. Needs the bearer token.
//! - `GET /v1/invoices/{id}`: status and payment link. The id is the capability.
//! - `GET /v1/invoices/{id}/qr.svg`: the payment QR code.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maya_address::Network;
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::chain::ChainSource;
use crate::error::{CheckoutError, Result};
use crate::invoice::{self, Invoice, NewInvoice};
use crate::ssrf;
use crate::store::Store;
use crate::uri::{payment_qr_svg, payment_uri};

/// Largest request body accepted, in bytes. A checkout request is a few hundred.
const MAX_BODY_BYTES: usize = 8 * 1024;

/// Shared state for every handler.
pub struct AppState {
    /// The ledger.
    pub store: Arc<Mutex<Store>>,
    /// Node access, used to fix an invoice's creation height.
    pub chain: Arc<dyn ChainSource>,
    /// Network the merchant address belongs to.
    pub network: Network,
    /// Merchant receiving address, 32 bytes.
    pub pay_to: [u8; 32],
    /// Bearer token for invoice creation.
    pub api_token: Zeroizing<Vec<u8>>,
}

/// The routes, bound to `state`.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/invoices", post(create))
        .route("/v1/invoices/{id}", get(view))
        .route("/v1/invoices/{id}/qr.svg", get(qr))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

#[derive(Deserialize)]
struct CreateBody {
    amount: String,
    memo: String,
    expires_in_blocks: u64,
    webhook_url: String,
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<()> {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(CheckoutError::Unauthorized)?;
    // Constant-time: a timing difference must not reveal the token byte by byte.
    if bool::from(presented.as_bytes().ct_eq(&state.api_token)) {
        Ok(())
    } else {
        Err(CheckoutError::Unauthorized)
    }
}

async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<Value>)> {
    authorize(&state, &headers)?;
    let req = NewInvoice {
        amount: invoice::amount::parse(&body.amount)?,
        memo: body.memo,
        expires_in_blocks: body.expires_in_blocks,
        webhook_url: body.webhook_url,
    };
    invoice::validate_memo(&req.memo)?;
    invoice::validate_expiry(req.expires_in_blocks)?;
    ssrf::vet(&req.webhook_url)
        .await
        .map_err(|e| CheckoutError::BadRequest(format!("webhook_url: {e}")))?;

    let id = invoice::new_id()?;
    let mut store = state.store.lock().await;
    // The tip is read while the lock is held, so no block can be scanned
    // before the invoice exists and then credited to it.
    let tip = state.chain.tip().await?;
    let inv = Invoice::open(id, state.pay_to, &req, tip.height)?;
    store.insert(inv.clone())?;
    Ok((StatusCode::CREATED, Json(render(&state, &inv))))
}

async fn view(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let store = state.store.lock().await;
    let inv = store.find(&id).ok_or(CheckoutError::NotFound)?;
    Ok(Json(render(&state, inv)))
}

async fn qr(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Response> {
    let uri = {
        let store = state.store.lock().await;
        let inv = store.find(&id).ok_or(CheckoutError::NotFound)?;
        link(&state, inv)
    };
    let svg = payment_qr_svg(&uri)?;
    Ok(([(header::CONTENT_TYPE, "image/svg+xml")], Body::from(svg)).into_response())
}

fn link(state: &AppState, inv: &Invoice) -> String {
    payment_uri(state.network, &inv.recipient, inv.amount, &inv.memo)
}

/// The public view of an invoice. The webhook URL and the signing details are
/// left out: they are the shop's business, not the customer's.
fn render(state: &AppState, inv: &Invoice) -> Value {
    let payment = inv.payment.as_ref().map(|p| {
        json!({
            "txid": p.txid,
            "index": p.index,
            "height": p.height,
            "amount_paid": p.amount_paid.to_string(),
            "overpaid": p.amount_paid > inv.amount,
        })
    });
    json!({
        "id": inv.id,
        "status": inv.status,
        "amount": inv.amount.to_string(),
        "memo": inv.memo,
        "created_height": inv.created_height,
        "expires_at_height": inv.expires_at_height,
        "pay_uri": link(state, inv),
        "payment": payment,
        "delivery": inv.delivery,
    })
}
