//! Response and request shapes, as the gateway's REST routes send them.

use primitive_types::U256;
use serde::{Deserialize, Deserializer, Serialize};

/// Amounts travel as exact decimal strings (256-bit, ADR-046). Parsing into
/// `U256` keeps every digit: a JSON number would round past 2^53.
fn decimal_u256<'de, D: Deserializer<'de>>(deserializer: D) -> Result<U256, D::Error> {
    let text = String::deserialize(deserializer)?;
    U256::from_dec_str(&text).map_err(|e| serde::de::Error::custom(format!("{text:?}: {e:?}")))
}

/// `GET /health`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Health {
    /// `"ok"` when the gateway is serving. Says nothing about the node.
    pub status: String,
}

/// `GET /v1/chain`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChainInfo {
    /// Hex genesis block id: the signing tag (ADR-036).
    pub genesis: String,
    /// Network name (`maya-testnet-1`), when the node was given one.
    pub chain_id: Option<String>,
    /// The node's tip height when it answered.
    #[serde(default)]
    pub height: u64,
}

/// `GET /v1/fees` (ADR-029).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FeeInfo {
    /// Whether this chain charges fees at all.
    pub active: bool,
    /// Base fee per serialized byte for the next block.
    pub base_fee: u64,
    /// Hex address the fee is paid to.
    pub collector: String,
}

/// `GET /v1/accounts/{address}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Balance {
    /// Spendable amount in base units.
    #[serde(deserialize_with = "decimal_u256")]
    pub balance: U256,
    /// Next valid nonce.
    pub nonce: u64,
}

/// `GET /v1/supply`, in base units.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Supply {
    /// Units in circulation.
    #[serde(deserialize_with = "decimal_u256")]
    pub circulating: U256,
    /// Units issued in total.
    #[serde(deserialize_with = "decimal_u256")]
    pub total: U256,
}

/// `POST /v1/transactions` body.
#[derive(Debug, Serialize)]
pub(crate) struct RawTransaction<'a> {
    /// Hex-encoded signed transaction.
    pub raw: &'a str,
}

/// `POST /v1/transactions` answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct TransactionAccepted {
    /// Hex transaction hash. Accepted into the mempool, not yet in a block.
    pub hash: String,
}

/// `POST /v1/sealed` body.
#[derive(Debug, Serialize)]
pub(crate) struct SealedSubmission<'a> {
    /// Hex threshold ciphertext, without `0x`.
    pub ciphertext: &'a str,
}

/// `POST /v1/sealed` answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SealedAccepted {
    /// Hex digest for correlating the submission.
    pub id: String,
    /// Ciphertext length in bytes, echoed for framing checks.
    pub bytes: usize,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn an_amount_above_two_to_the_sixty_fourth_survives_exactly() {
        // 2^128 - 1 is not a JSON-safe integer and does not fit a u64.
        let text = "340282366920938463463374607431768211455";
        let json = format!(r#"{{"balance":"{text}","nonce":7}}"#);
        let balance: Balance = serde_json::from_str(&json).expect("parses");
        assert_eq!(balance.balance.to_string(), text);
        assert_eq!(balance.nonce, 7);
    }

    #[test]
    fn a_number_where_a_decimal_string_belongs_is_refused() {
        let result: Result<Balance, _> = serde_json::from_str(r#"{"balance":12,"nonce":0}"#);
        assert!(result.is_err());
    }
}
