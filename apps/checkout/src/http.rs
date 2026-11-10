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
use crate::invoice::{self, Delivery, Invoice, NewInvoice};
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
    /// Budget for the public, unauthenticated reads.
    pub reads: ReadLimiter,
}

/// Public reads admitted per second, across all callers. The node's own
/// per-address limit is 50 per second; the service keeps the same bound so a
/// flood cannot hold the ledger lock for long.
pub const PUBLIC_READS_PER_SEC: u32 = 50;

/// A fixed one-second window. Simple on purpose: a flood is refused, not queued.
#[derive(Debug, Default)]
pub struct ReadLimiter {
    /// The window's second and how many reads it has admitted.
    window: std::sync::Mutex<(u64, u32)>,
}

impl ReadLimiter {
    /// Whether a read at `now_unix` fits in the budget.
    pub fn admit(&self, now_unix: u64) -> bool {
        let mut window = self.window.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if window.0 != now_unix {
            *window = (now_unix, 0);
        }
        if window.1 >= PUBLIC_READS_PER_SEC {
            return false;
        }
        window.1 += 1;
        true
    }
}

/// The routes, bound to `state`.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/invoices", post(create))
        .route("/v1/invoices/{id}", get(view))
        .route("/v1/invoices/{id}/qr.svg", get(qr))
        .route("/v1/invoices/{id}/resend", post(resend))
        .route("/v1/unmatched", get(unmatched))
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
    // The tip is read before the lock, never while holding it: a hung node must
    // not freeze every other request. A block that lands between this read and
    // the insert can hold an exact-amount payment the scanner records as
    // unmatched. That fails safe: the merchant reviews it, nothing settles wrongly.
    let tip = state.chain.tip().await?;
    let inv = Invoice::open(id, state.pay_to, &req, tip.height)?;
    let mut store = state.store.lock().await;
    store.insert(inv.clone())?;
    Ok((StatusCode::CREATED, Json(render(&state, &inv))))
}

/// Admits one public read, or refuses it when the per-second budget is spent.
fn admit_read(state: &AppState) -> Result<()> {
    if state.reads.admit(unix_now()) { Ok(()) } else { Err(CheckoutError::RateLimited) }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

async fn view(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    admit_read(&state)?;
    let store = state.store.lock().await;
    let inv = store.find(&id).ok_or(CheckoutError::NotFound)?;
    Ok(Json(render(&state, inv)))
}

/// The merchant's view of payments that settled nothing: refund or assign by hand.
async fn unmatched(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Result<Json<Value>> {
    authorize(&state, &headers)?;
    let store = state.store.lock().await;
    let rows: Vec<Value> = store
        .ledger()
        .unmatched
        .iter()
        .map(|u| {
            json!({
                "txid": u.txid,
                "index": u.index,
                "height": u.height,
                "recipient": hex::encode(u.recipient),
                "amount": u.amount.to_string(),
                "reason": u.reason,
            })
        })
        .collect();
    Ok(Json(Value::Array(rows)))
}

/// Puts a webhook that ran out of attempts back in the queue. Refuses one the
/// shop already acknowledged, so a second `invoice.confirmed` cannot be sent.
async fn resend(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    authorize(&state, &headers)?;
    let mut store = state.store.lock().await;
    let inv = store
        .ledger_mut()
        .invoices
        .iter_mut()
        .find(|inv| inv.id == id)
        .ok_or(CheckoutError::NotFound)?;
    match inv.delivery {
        Delivery::Failed => {
            inv.delivery = Delivery::Due;
            inv.attempts = 0;
            inv.next_attempt_unix = 0;
        }
        Delivery::Delivered => {
            return Err(CheckoutError::Conflict("this webhook was already delivered".into()));
        }
        Delivery::NotDue | Delivery::Due => {}
    }
    let view = render(&state, inv);
    store.commit()?;
    Ok(Json(view))
}

async fn qr(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Response> {
    admit_read(&state)?;
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
