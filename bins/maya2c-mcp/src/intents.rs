//! Intent-market tools (P-30, ADR-139): post a signed intent, offer and list
//! quotes, and plan the HTLC settlement.
//!
//! The book lives in this process only. Nothing here signs or submits: the
//! intent arrives already signed by the maker's wallet, and the plan lists the
//! `HtlcLock`/`HtlcClaim`/`HtlcRefund` steps for each party's own wallet to
//! sign. Heights come from the node, so "expired" means expired on the chain
//! the assistant is reading, not on this machine's clock.

use maya_htlc_lattice::HashFunction;
use maya_intent_solver::{
    Action, AssetId, Intent, IntentBook, IntentId, Leg, MAX_RECIPIENT_BYTES, Party, Quote,
    SettlementPlan, SignedIntent, plan_settlement,
};
use primitive_types::U256;
use serde_json::{Value, json};

use crate::node::Node;
use crate::tools::{base_units, hex32, text_arg, to_u256};

/// Hex characters in a hybrid public key and signature, with room to spare;
/// the exact lengths are the verifier's to check.
const MAX_KEY_HEX: usize = 2 * 2_048;
const MAX_SIGNATURE_HEX: usize = 2 * 12_000;

/// The book, created on first use for the genesis the node reports.
#[derive(Default)]
pub struct Intents {
    book: Option<([u8; 32], IntentBook)>,
}

pub fn list() -> Vec<Value> {
    let id = json!({"type": "string", "description": "64 hex characters"});
    vec![
        json!({"name": "intent_post", "description": "Post a maker's signed intent (hybrid ML-DSA-65 + SLH-DSA). Refuses a replayed nonce, an expired deadline, a wrong network or a bad signature. Amounts are base-unit decimal strings.",
         "inputSchema": {"type": "object", "properties": {
             "intent": {"type": "object", "description": "maker, sell_asset, buy_asset (64 hex; 0…0 is the native coin), max_sell, buy_amount (decimal strings), buy_recipient (hex), deadline_height, nonce (integers)"},
             "public_key": {"type": "string"}, "signature": {"type": "string"}},
             "required": ["intent", "public_key", "signature"]}}),
        json!({"name": "intent_offer", "description": "Record a solver's quote: price + fee of the sell asset for the intent's buy amount. Refused if past the maker's limit, after the deadline, or not lower than this solver's earlier quote.",
         "inputSchema": {"type": "object", "properties": {
             "intent_id": id, "solver": id, "pay_to": {"type": "string", "description": "hex: where the maker's lock pays"},
             "price": {"type": "string"}, "fee": {"type": "string"}},
             "required": ["intent_id", "solver", "pay_to", "price", "fee"]}}),
        json!({"name": "intent_quotes", "description": "Quotes for an intent, best first (lowest total, then lowest solver id), and the best one.",
         "inputSchema": {"type": "object", "properties": {"intent_id": id}, "required": ["intent_id"]}}),
        json!({"name": "intent_settle_plan", "description": "Plan the HTLC settlement against the best quote: the two locks, their expiries and the claim and refund steps. The maker supplies only the digest and keeps the preimage. Nothing is signed or sent.",
         "inputSchema": {"type": "object", "properties": {
             "intent_id": id, "digest": id,
             "hash": {"type": "string", "enum": ["sha256", "sha3-256", "blake3"]},
             "margin": {"type": "integer", "minimum": 1, "description": "blocks each party is guaranteed to act in"}},
             "required": ["intent_id", "digest", "hash", "margin"]}}),
    ]
}

/// Runs an intent tool, or `None` if `name` is not one.
pub fn call(
    intents: &mut Intents,
    node: &mut Node,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    let run = match name {
        "intent_post" => post,
        "intent_offer" => offer,
        "intent_quotes" => quotes,
        "intent_settle_plan" => settle_plan,
        _ => return None,
    };
    Some(chain_view(node).and_then(|(genesis, height)| {
        let book = intents.book_for(genesis)?;
        run(book, args, height)
    }))
}

impl Intents {
    fn book_for(&mut self, genesis: [u8; 32]) -> Result<&mut IntentBook, String> {
        let (network, book) = self
            .book
            .get_or_insert_with(|| (genesis, IntentBook::new(genesis)));
        if *network != genesis {
            return Err("the node now reports another genesis; restart the server".into());
        }
        Ok(book)
    }
}

/// `(genesis, height)` from the node.
fn chain_view(node: &mut Node) -> Result<([u8; 32], u64), String> {
    let info = node.call("get_chain_info", &json!([]))?;
    let genesis = hex32(info["genesis"].as_str().unwrap_or(""), "node genesis")?;
    let height = info["height"]
        .as_u64()
        .ok_or_else(|| "node reported no height".to_string())?;
    Ok((genesis, height))
}

fn post(book: &mut IntentBook, args: &Value, height: u64) -> Result<Value, String> {
    let signed = SignedIntent {
        intent: intent_arg(&args["intent"], book_network(args)?)?,
        public_key: bounded_hex(text_arg(args, "public_key")?, MAX_KEY_HEX, "public_key")?,
        signature: bounded_hex(text_arg(args, "signature")?, MAX_SIGNATURE_HEX, "signature")?,
    };
    let id = book.post(signed, height).map_err(|e| e.to_string())?;
    Ok(json!({"intent_id": hex::encode(id), "height": height}))
}

/// The network the intent claims; the book refuses it if it is not this one.
fn book_network(args: &Value) -> Result<[u8; 32], String> {
    hex32(text_arg(&args["intent"], "network")?, "intent.network")
}

fn intent_arg(v: &Value, network: [u8; 32]) -> Result<Intent, String> {
    Ok(Intent {
        network,
        maker: hex32(text_arg(v, "maker")?, "intent.maker")?,
        sell_asset: AssetId(hex32(text_arg(v, "sell_asset")?, "intent.sell_asset")?),
        max_sell: amount(v, "max_sell")?,
        buy_asset: AssetId(hex32(text_arg(v, "buy_asset")?, "intent.buy_asset")?),
        buy_amount: amount(v, "buy_amount")?,
        buy_recipient: bounded_hex(
            text_arg(v, "buy_recipient")?,
            2 * MAX_RECIPIENT_BYTES,
            "intent.buy_recipient",
        )?,
        deadline_height: integer(v, "deadline_height")?,
        nonce: integer(v, "nonce")?,
    })
}

fn offer(book: &mut IntentBook, args: &Value, height: u64) -> Result<Value, String> {
    let quote = Quote {
        intent_id: intent_id(args)?,
        solver: hex32(text_arg(args, "solver")?, "solver")?,
        pay_to: bounded_hex(text_arg(args, "pay_to")?, 2 * MAX_RECIPIENT_BYTES, "pay_to")?,
        price: amount(args, "price")?,
        fee: amount(args, "fee")?,
    };
    book.offer(quote, height).map_err(|e| e.to_string())?;
    Ok(json!({"accepted": true, "height": height}))
}

fn quotes(book: &mut IntentBook, args: &Value, _height: u64) -> Result<Value, String> {
    let id = intent_id(args)?;
    let quotes = book.quotes(&id).map_err(|e| e.to_string())?;
    let rendered: Vec<Value> = quotes.iter().map(quote_json).collect();
    Ok(json!({"intent_id": hex::encode(id), "best": rendered.first(), "quotes": rendered}))
}

fn settle_plan(book: &mut IntentBook, args: &Value, height: u64) -> Result<Value, String> {
    let id = intent_id(args)?;
    let function = match text_arg(args, "hash")? {
        "sha256" => HashFunction::Sha256,
        "sha3-256" => HashFunction::Sha3_256,
        "blake3" => HashFunction::Blake3,
        other => return Err(format!("hash {other} is not sha256, sha3-256 or blake3")),
    };
    let digest = hex32(text_arg(args, "digest")?, "digest")?;
    let margin = integer(args, "margin")?;
    let best = book.best(&id).map_err(|e| e.to_string())?;
    let intent = book
        .intent(&id)
        .ok_or_else(|| "unknown intent".to_string())?;
    let plan = plan_settlement(&intent.intent, &best, function, digest, height, margin)
        .map_err(|e| e.to_string())?;
    Ok(plan_json(&plan, args["hash"].as_str().unwrap_or("")))
}

fn plan_json(plan: &SettlementPlan, hash: &str) -> Value {
    let steps: Vec<Value> = plan
        .steps()
        .iter()
        .map(|s| {
            json!({"party": party(s.party), "action": action(s.action), "lock_of": party(s.lock_of),
                   "from_height": s.from_height, "before_height": s.before_height})
        })
        .collect();
    json!({
        "intent_id": hex::encode(plan.intent_id),
        "solver": hex::encode(plan.solver),
        "hash": hash,
        "digest": hex::encode(plan.digest),
        "lock_by": plan.lock_by,
        "maker_leg": leg_json(&plan.maker_leg),
        "solver_leg": leg_json(&plan.solver_leg),
        "steps": steps,
        "note": "Each party signs its own HtlcLock/HtlcClaim/HtlcRefund with its own wallet. The solver locks only after seeing the maker's lock on chain. A leg not on Maya2C is an HTLC on that asset's chain under the same digest; its expiry here is in Maya2C heights."
    })
}

fn leg_json(leg: &Leg) -> Value {
    json!({
        "asset": hex::encode(leg.asset.0),
        "recipient": hex::encode(&leg.recipient),
        "amount": leg.amount.to_string(),
        "expiry_height": leg.expiry_height,
        "settles_on": if leg.on_maya2c { "maya2c HtlcLock" } else { "other chain HTLC" },
    })
}

fn quote_json(q: &Quote) -> Value {
    let total = q
        .total()
        .map_or_else(|_| "overflow".to_string(), |t| t.to_string());
    json!({"solver": hex::encode(q.solver), "pay_to": hex::encode(&q.pay_to),
           "price": q.price.to_string(), "fee": q.fee.to_string(), "total": total})
}

const fn party(p: Party) -> &'static str {
    match p {
        Party::Maker => "maker",
        Party::Solver => "solver",
    }
}

const fn action(a: Action) -> &'static str {
    match a {
        Action::Lock => "lock",
        Action::Claim => "claim",
        Action::Refund => "refund",
    }
}

fn intent_id(args: &Value) -> Result<IntentId, String> {
    hex32(text_arg(args, "intent_id")?, "intent_id")
}

fn amount(v: &Value, key: &str) -> Result<U256, String> {
    base_units(text_arg(v, key)?, key).map(to_u256)
}

fn integer(v: &Value, key: &str) -> Result<u64, String> {
    v[key]
        .as_u64()
        .ok_or_else(|| format!("{key} is required (a non-negative integer)"))
}

fn bounded_hex(text: &str, max: usize, what: &str) -> Result<Vec<u8>, String> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() > max {
        return Err(format!("{what} is longer than {max} hex characters"));
    }
    hex::decode(digits).map_err(|e| format!("{what}: {e}"))
}
