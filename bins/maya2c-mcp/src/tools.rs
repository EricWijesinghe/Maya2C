//! The tools an assistant can call (ADR-084). Chain reads go through
//! [`Node::call`]; everything else is computed locally. Every input is
//! validated before anything leaves the process. No tool signs or submits:
//! `draft_transfer` returns the transaction unsigned, for the person's own
//! wallet to sign.

use custom_l1_node::core::Transaction;
use custom_l1_node::core::transaction::MAX_TX_BYTES;
use custom_l1_node::crypto::hybrid::HYBRID_SIGNATURE_LENGTH;
use maya_address::Network;
use maya_amount::{Amount, COIN_DECIMALS, ONE_COIN};
use maya_clear_sign::{Effect, review};
use primitive_types::U256;
use serde_json::{Value, json};

use crate::node::Node;

/// Longest address text accepted (a maya1 address is about 63 characters).
const MAX_ADDRESS_TEXT: usize = 128;
/// Longest base-unit amount: 2^256 - 1 has 78 digits.
const MAX_AMOUNT_TEXT: usize = 80;
/// Longest coin amount text: 78 whole digits, a point, 24 fraction digits.
const MAX_COIN_TEXT: usize = 80;
/// Hex characters in a 32-byte id or a transaction id.
const ID_HEX_LEN: usize = 64;

pub fn list() -> Value {
    json!([
        {"name": "get_height", "description": "Current chain height, chain id and genesis (the chain tag every signature commits to).",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "get_block", "description": "A block by height: header, transaction count and transaction ids.",
         "inputSchema": {"type": "object", "properties": {"height": {"type": "integer", "minimum": 0}}, "required": ["height"]}},
        {"name": "get_tx", "description": "Find a transaction by id inside the block at a given height (the node has no lookup by id, so the height is required).",
         "inputSchema": {"type": "object", "properties": {"txid": {"type": "string", "description": "64 hex characters"}, "height": {"type": "integer", "minimum": 0}}, "required": ["txid", "height"]}},
        {"name": "get_balance", "description": "Spendable balance (base units, a decimal integer; 24 decimals per coin) and next nonce of an account.",
         "inputSchema": {"type": "object", "properties": {"address": {"type": "string", "description": "maya1... text or 64 hex characters"}}, "required": ["address"]}},
        {"name": "get_validators", "description": "The current validator set and staking record.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "get_supply", "description": "Total, circulating and burned supply, in base units.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "estimate_fee", "description": "Minimum fee for a standard one-output transfer, in base units, from the node's current base fee.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "draft_transfer", "description": "Draft a transfer for the person to sign with their own wallet. Returns the unsigned transaction (hex) and a plain-language summary. Nothing is signed or sent.",
         "inputSchema": {"type": "object", "properties": {
             "from": {"type": "string", "description": "sender address: maya1... or 64 hex"},
             "to": {"type": "string", "description": "recipient address: maya1... or 64 hex"},
             "amount": {"type": "string", "description": "coins as a decimal string, at most 24 fraction digits, e.g. \"1.5\""}},
             "required": ["from", "to", "amount"]}},
        {"name": "decode_transaction", "description": "Decode a hex-encoded Maya2C transaction with the node's decoder; returns version fields, txid and sender, or the decoder's error.",
         "inputSchema": {"type": "object", "properties": {"hex": {"type": "string"}}, "required": ["hex"]}},
        {"name": "review_intent", "description": "Clear-signing review: compare the effects an app claims with the effects a simulation shows; returns warnings, most severe first. Amounts are base units.",
         "inputSchema": {"type": "object", "properties": {"claimed": {"type": "array"}, "simulated": {"type": "array"}}, "required": ["claimed", "simulated"]}},
        {"name": "explain_error", "description": "Explain a node error kind (e.g. InvalidNonce) in plain language, with the spec rule it comes from.",
         "inputSchema": {"type": "object", "properties": {"kind": {"type": "string"}}, "required": ["kind"]}}
    ])
}

/// Runs one tool. The error string is shown to the assistant as the tool's text.
pub fn call(node: &mut Node, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "get_height" => get_height(node),
        "get_block" => get_block(node, args),
        "get_tx" => get_tx(node, args),
        "get_balance" => get_balance(node, args),
        "get_validators" => Ok(json!({"validators": node.call("get_validators", &json!([]))?})),
        "get_supply" => node.call("get_supply", &json!([])),
        "estimate_fee" => estimate_fee(node),
        "draft_transfer" => draft_transfer(node, args),
        "decode_transaction" => decode(text_arg(args, "hex")?),
        "review_intent" => review_intent(args),
        "explain_error" => explain(text_arg(args, "kind")?),
        other => Err(format!("unknown tool {other}")),
    }
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .map(str::trim)
        .ok_or_else(|| format!("{key} is required (a string)"))
}

fn height_arg(args: &Value) -> Result<u64, String> {
    args["height"]
        .as_u64()
        .ok_or_else(|| "height is required (a non-negative integer)".to_string())
}

/// 64 hex characters, with an optional `0x`, as 32 bytes.
fn hex32(text: &str, what: &str) -> Result<[u8; 32], String> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() != ID_HEX_LEN || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{what} must be {ID_HEX_LEN} hex characters"));
    }
    let bytes = hex::decode(digits).map_err(|e| format!("{what}: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| format!("{what} must be 32 bytes"))
}

/// An account from `maya1...` text or 64 hex characters.
fn account(text: &str, what: &str) -> Result<[u8; 32], String> {
    if text.len() > MAX_ADDRESS_TEXT {
        return Err(format!(
            "{what} is longer than {MAX_ADDRESS_TEXT} characters"
        ));
    }
    let looks_hex = text.strip_prefix("0x").unwrap_or(text);
    if looks_hex.len() == ID_HEX_LEN {
        return hex32(text, what);
    }
    maya_address::decode_any(text)
        .map(|(_, id)| id)
        .map_err(|e| format!("{what} is not a maya1 address or 64 hex characters: {e}"))
}

/// Base units: a non-empty string of decimal digits.
fn base_units(text: &str, what: &str) -> Result<Amount, String> {
    if text.is_empty() || text.len() > MAX_AMOUNT_TEXT || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!("{what} is a decimal integer in base units"));
    }
    let ten = Amount::from_u64(10);
    text.bytes()
        .try_fold(Amount::ZERO, |acc, b| {
            acc.checked_mul(ten)?
                .checked_add(Amount::from_u64(u64::from(b - b'0')))
        })
        .ok_or_else(|| format!("{what} exceeds 2^256 - 1"))
}

/// Coins with at most [`COIN_DECIMALS`] fraction digits, as base units.
/// Extra digits are refused, not rounded: a rounded amount is a different
/// payment from the one the person typed.
fn coins(text: &str, what: &str) -> Result<Amount, String> {
    if text.len() > MAX_COIN_TEXT {
        return Err(format!("{what} is longer than {MAX_COIN_TEXT} characters"));
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > COIN_DECIMALS {
        return Err(format!(
            "{what} has more than {COIN_DECIMALS} decimal places"
        ));
    }
    let whole_units = base_units(whole, what)?;
    let fraction_units = base_units(&format!("{fraction:0<COIN_DECIMALS$}"), what)?;
    whole_units
        .checked_mul(Amount::from_u128(ONE_COIN))
        .and_then(|units| units.checked_add(fraction_units))
        .ok_or_else(|| format!("{what} exceeds 2^256 - 1 base units"))
}

fn to_u256(amount: Amount) -> U256 {
    U256::from_little_endian(&amount.to_le_bytes())
}

fn output(amount: Amount, recipient: [u8; 32]) -> custom_l1_node::TxOutput {
    custom_l1_node::TxOutput {
        amount: to_u256(amount),
        recipient,
    }
}

/// The outputs of a transfer: the payment, then the fee to the collector when
/// the chain charges fees. Same shape as `l1-wallet` builds.
fn outputs(
    fees_active: bool,
    to: [u8; 32],
    amount: Amount,
    fee: Amount,
    collector: [u8; 32],
) -> Vec<custom_l1_node::TxOutput> {
    let mut out = vec![output(amount, to)];
    if fees_active {
        out.push(output(fee, collector));
    }
    out
}

/// Encoded size of the transfer once signed. Outputs are fixed width, so the
/// size does not depend on the amounts; the signature is a fixed-length field
/// that an unsigned encoding leaves out.
fn signed_size(outputs: Vec<custom_l1_node::TxOutput>) -> usize {
    Transaction::new(vec![], outputs, 0).to_bytes().len() + HYBRID_SIGNATURE_LENGTH
}

/// `(fees active, base fee per byte, collector)` from the node.
fn fee_rules(node: &mut Node) -> Result<(bool, Amount, [u8; 32]), String> {
    let info = node.call("get_fee_info", &json!([]))?;
    let active = info["active"]
        .as_bool()
        .ok_or("node: fee info has no active flag")?;
    let base = base_units(info["base_fee"].as_str().unwrap_or(""), "node base_fee")?;
    let collector = hex32(info["collector"].as_str().unwrap_or(""), "node collector")?;
    Ok((active, base, collector))
}

/// `base fee x size`, saturating like the node's own requirement.
fn fee_for(base: Amount, size: usize) -> Amount {
    base.checked_mul(Amount::from_u128(size as u128))
        .unwrap_or(Amount::MAX)
}

fn get_height(node: &mut Node) -> Result<Value, String> {
    let info = node.call("get_chain_info", &json!([]))?;
    Ok(json!({"height": info["height"], "chain_id": info["chain_id"], "genesis": info["genesis"]}))
}

fn get_block(node: &mut Node, args: &Value) -> Result<Value, String> {
    let height = height_arg(args)?;
    let info = node.call("get_block_by_height", &json!([height]))?;
    let txids: Vec<&Value> = info["transactions"]
        .as_array()
        .map(|list| list.iter().map(|t| &t["txid"]).collect())
        .unwrap_or_default();
    Ok(
        json!({"height": info["height"], "header": info["header"], "tx_count": txids.len(), "txids": txids}),
    )
}

fn get_tx(node: &mut Node, args: &Value) -> Result<Value, String> {
    let wanted = text_arg(args, "txid")?.to_ascii_lowercase();
    hex32(&wanted, "txid")?;
    let height = height_arg(args)?;
    let info = node.call("get_block_by_height", &json!([height]))?;
    let found = info["transactions"].as_array().and_then(|list| {
        list.iter()
            .find(|t| t["txid"].as_str() == Some(wanted.as_str()))
    });
    Ok(match found {
        Some(tx) => json!({"found": true, "height": height, "transaction": tx}),
        None => json!({"found": false, "height": height}),
    })
}

fn get_balance(node: &mut Node, args: &Value) -> Result<Value, String> {
    let id = account(text_arg(args, "address")?, "address")?;
    node.call("get_balance", &json!([hex::encode(id)]))
}

fn estimate_fee(node: &mut Node) -> Result<Value, String> {
    let (active, base, collector) = fee_rules(node)?;
    let zero = [0u8; 32];
    let size = signed_size(outputs(active, zero, Amount::ZERO, Amount::ZERO, collector));
    Ok(json!({
        "fees_active": active,
        "base_fee_per_byte": base.to_string(),
        "transfer_size_bytes": size,
        "fee_base_units": fee_for(base, size).to_string(),
    }))
}

fn draft_transfer(node: &mut Node, args: &Value) -> Result<Value, String> {
    let from = account(text_arg(args, "from")?, "from")?;
    let to = account(text_arg(args, "to")?, "to")?;
    let coin_text = text_arg(args, "amount")?;
    let amount = coins(coin_text, "amount")?;
    if amount.is_zero() {
        return Err("amount must be above zero".into());
    }

    let chain = node.call("get_chain_info", &json!([]))?;
    let chain_id = chain["chain_id"].as_str().map(str::to_string);
    let genesis = chain["genesis"].as_str().unwrap_or("").to_string();
    let account_info = node.call("get_balance", &json!([hex::encode(from)]))?;
    let nonce = account_info["nonce"]
        .as_u64()
        .ok_or("node: account has no nonce")?;
    let balance = base_units(
        account_info["balance"].as_str().unwrap_or(""),
        "node balance",
    )?;

    let (active, base, collector) = fee_rules(node)?;
    let size = signed_size(outputs(active, to, amount, Amount::ZERO, collector));
    let fee = fee_for(base, size);
    let total = amount
        .checked_add(fee)
        .ok_or("amount plus fee exceeds 2^256 - 1")?;
    let unsigned = Transaction::new(vec![], outputs(active, to, amount, fee, collector), nonce);

    let network = chain_id.as_deref().and_then(Network::for_chain_id);
    let show =
        |id: [u8; 32]| network.map_or_else(|| hex::encode(id), |n| maya_address::encode(n, &id));
    let sufficient = balance >= total;
    let name_note = match &chain_id {
        Some(name) => format!("network {name}"),
        None => "network name unknown: compare chain_tag with the network you mean to use".into(),
    };
    let summary = format!(
        "Send {coin_text} coins ({amount} base units) to {}. Fee {fee} base units. Total {total} from {}, nonce {nonce}, {name_note}. Balance {balance}: {}.",
        show(to),
        show(from),
        if sufficient {
            "enough"
        } else {
            "NOT enough for this payment"
        },
    );
    Ok(json!({
        "signed": false,
        "unsigned_tx_hex": hex::encode(unsigned.to_bytes()),
        "chain_tag": genesis,
        "network": chain_id,
        "from": hex::encode(from),
        "to": hex::encode(to),
        "nonce": nonce,
        "amount_base_units": amount.to_string(),
        "fee_base_units": fee.to_string(),
        "total_base_units": total.to_string(),
        "balance_base_units": balance.to_string(),
        "sufficient_balance": sufficient,
        "summary": summary,
        "next_step": "Check the summary, then sign unsigned_tx_hex with your own wallet for chain_tag and submit it there. This server cannot sign or submit.",
    }))
}

fn decode(hex_str: &str) -> Result<Value, String> {
    if hex_str.len() > 2 * MAX_TX_BYTES {
        return Err(format!(
            "transaction hex is longer than {MAX_TX_BYTES} bytes"
        ));
    }
    let bytes = hex::decode(hex_str).map_err(|e| format!("not hex: {e}"))?;
    let version = *bytes.first().ok_or("empty transaction")?;
    let tx = Transaction::from_bytes(&bytes).map_err(|e| e.to_string())?;
    let outputs: Vec<Value> = tx
        .outputs
        .iter()
        .map(|o| json!({"to": hex::encode(o.recipient), "amount": o.amount.to_string()}))
        .collect();
    Ok(
        json!({"version": version, "nonce": tx.nonce.to_string(), "outputs": outputs, "signed": tx.signature.is_some(),
              "txid": hex::encode(tx.txid()), "sender": hex::encode(tx.sender())}),
    )
}

/// `{"type": "transfer", "token": hex, "to": hex, "amount": "decimal"}` and the like.
fn effect(v: &Value) -> Result<Effect, String> {
    let field = |k: &str| account(v[k].as_str().unwrap_or(""), k);
    let amount = || base_units(v["amount"].as_str().unwrap_or(""), "amount");
    Ok(match v["type"].as_str().ok_or("effect needs a type")? {
        "transfer" => Effect::Transfer {
            token: field("token")?,
            to: field("to")?,
            amount: amount()?,
        },
        "approve" => Effect::Approve {
            token: field("token")?,
            spender: field("spender")?,
            amount: amount()?,
        },
        "approve_all" => Effect::ApproveAll {
            collection: field("collection")?,
            operator: field("operator")?,
        },
        "permit" => Effect::Permit {
            token: field("token")?,
            spender: field("spender")?,
            amount: amount()?,
        },
        "upgrade" => Effect::Upgrade {
            contract: field("contract")?,
        },
        "set_owner" => Effect::SetOwner {
            target: field("target")?,
            owner: field("owner")?,
        },
        "call" => Effect::Call {
            contract: field("contract")?,
            method: v["method"].as_str().unwrap_or("").to_string(),
        },
        other => return Err(format!("unknown effect type {other}")),
    })
}

fn review_intent(args: &Value) -> Result<Value, String> {
    let list = |k: &str| -> Result<Vec<Effect>, String> {
        args[k]
            .as_array()
            .ok_or(format!("{k} must be an array"))?
            .iter()
            .map(effect)
            .collect()
    };
    let warnings = review(
        &list("claimed")?,
        &list("simulated")?,
        &maya_clear_sign::Context::default(),
    );
    Ok(json!(
        warnings
            .iter()
            .map(|w| format!("{w:?}"))
            .collect::<Vec<_>>()
    ))
}

fn explain(kind: &str) -> Result<Value, String> {
    let (text, rule) = match kind {
        "InvalidNonce" => (
            "The transaction's nonce is not the sender's next nonce: it was already used (a replay) or one is missing before it. Resend with the nonce `get_balance` reports.",
            "STF-1",
        ),
        "InsufficientBalance" => (
            "The sender's balance is below the sum of the outputs.",
            "STF-3",
        ),
        "BalanceOverflow" => (
            "An amount would exceed the largest representable amount (2^256 - 1, ADR-046).",
            "STF-2, STF-4, STF-5",
        ),
        "BadSignature" | "SignatureVerification" | "HashSignatureVerification" => (
            "One of the two signatures (ML-DSA-65 or SLH-DSA) does not verify; both must.",
            "TX-1",
        ),
        "UpgradeRequired" => (
            "The node reached a protocol version or feature bits it does not implement. Install a newer binary; it resumes where it stopped.",
            "spec §4, ADR-020",
        ),
        "Decode" => (
            "The bytes are not a well-formed transaction frame: wrong version, bad length, or trailing bytes.",
            "ENC-1 … ENC-7",
        ),
        other => return Err(format!("unknown error kind {other}")),
    };
    Ok(json!({"explanation": text, "rule": rule}))
}
