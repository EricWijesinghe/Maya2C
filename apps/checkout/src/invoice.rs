//! The invoice: what a shop asks to be paid, and where that payment stands.
//!
//! Amounts are 256-bit base units with 24 decimals (ADR-046, ADR-068), written
//! as decimal strings everywhere they cross a boundary.

use primitive_types::U256;
use serde::{Deserialize, Serialize};

use crate::error::{CheckoutError, Result};

/// Longest memo or order id, in bytes. The memo rides in a payment URI and a
/// QR code, so it stays short.
pub const MAX_MEMO_BYTES: usize = 64;
/// Shortest validity window: an invoice must be payable at least one block.
pub const MIN_EXPIRY_BLOCKS: u64 = 1;
/// Longest validity window. A ceiling, not a recommendation: without one, an
/// open invoice would reserve its amount indefinitely.
pub const MAX_EXPIRY_BLOCKS: u64 = 100_000;

/// Where an invoice is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Waiting for a payment.
    Pending,
    /// A matching payment is in a block but has not reached the confirmation
    /// depth yet.
    Detected,
    /// Paid and confirmed. The webhook is due or on its way.
    Confirmed,
    /// The window closed with no matching payment.
    Expired,
}

/// Whether the merchant webhook for an invoice has gone out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// The invoice is not confirmed, so there is nothing to send.
    NotDue,
    /// Confirmed, and waiting to be sent (or to be retried).
    Due,
    /// The merchant answered with a 2xx.
    Delivered,
    /// Every attempt failed. The merchant must look at the status endpoint.
    Failed,
}

/// The payment that settled an invoice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payment {
    /// Hex transaction id.
    pub txid: String,
    /// Index of the output within that transaction. With `txid`, this names
    /// one output, which can settle at most one invoice.
    pub index: u32,
    /// Block height that included the payment.
    pub height: u64,
    /// What the output actually paid. May exceed the invoice amount.
    #[serde(with = "amount")]
    pub amount_paid: U256,
}

/// A stored invoice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    /// 128 random bits, hex. The id is a capability: anyone holding it can
    /// read the invoice, so it is never guessable.
    pub id: String,
    /// Recipient address, 32 bytes, hex in storage.
    #[serde(with = "hex32")]
    pub recipient: [u8; 32],
    /// Amount due, base units.
    #[serde(with = "amount")]
    pub amount: U256,
    /// Order id or note from the shop. Carried in the payment URI only; it is
    /// not on chain.
    pub memo: String,
    /// Where the signed webhook goes. Checked again at every delivery.
    pub webhook_url: String,
    /// The chain tip when the invoice became visible. Only blocks above this
    /// height can pay it, so an old payment cannot be counted.
    pub created_height: u64,
    /// Last height that may include the payment.
    pub expires_at_height: u64,
    /// Where the invoice is now.
    pub status: Status,
    /// The settling payment, once detected.
    pub payment: Option<Payment>,
    /// Webhook state.
    pub delivery: Delivery,
    /// Failed delivery attempts so far.
    pub attempts: u32,
    /// Unix seconds before which the next attempt must not run.
    pub next_attempt_unix: u64,
}

/// What a shop sends to create an invoice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewInvoice {
    /// Amount in base units.
    pub amount: U256,
    /// Order id or note.
    pub memo: String,
    /// Blocks the invoice stays open.
    pub expires_in_blocks: u64,
    /// Webhook destination.
    pub webhook_url: String,
}

impl Invoice {
    /// Builds a pending invoice that becomes payable above `tip_height`.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::BadRequest`] for a zero amount, a bad memo or an
    /// expiry outside the allowed range.
    pub fn open(
        id: String,
        recipient: [u8; 32],
        req: &NewInvoice,
        tip_height: u64,
    ) -> Result<Self> {
        if req.amount.is_zero() {
            return Err(CheckoutError::BadRequest("amount must be above zero".into()));
        }
        validate_memo(&req.memo)?;
        validate_expiry(req.expires_in_blocks)?;
        Ok(Self {
            id,
            recipient,
            amount: req.amount,
            memo: req.memo.clone(),
            webhook_url: req.webhook_url.clone(),
            created_height: tip_height,
            expires_at_height: tip_height.saturating_add(req.expires_in_blocks),
            status: Status::Pending,
            payment: None,
            delivery: Delivery::NotDue,
            attempts: 0,
            next_attempt_unix: 0,
        })
    }
}

/// Rejects memos that are too long or contain control characters (a memo goes
/// into a URI and a QR code, and a shop's order id has no need of either).
///
/// # Errors
///
/// [`CheckoutError::BadRequest`] when the memo breaks either rule.
pub fn validate_memo(memo: &str) -> Result<()> {
    if memo.len() > MAX_MEMO_BYTES {
        return Err(CheckoutError::BadRequest(format!(
            "memo is longer than {MAX_MEMO_BYTES} bytes"
        )));
    }
    if memo.chars().any(char::is_control) {
        return Err(CheckoutError::BadRequest("memo contains control characters".into()));
    }
    Ok(())
}

/// Rejects expiry windows outside [`MIN_EXPIRY_BLOCKS`, `MAX_EXPIRY_BLOCKS`].
///
/// # Errors
///
/// [`CheckoutError::BadRequest`] when the window is out of range.
pub fn validate_expiry(blocks: u64) -> Result<()> {
    if !(MIN_EXPIRY_BLOCKS..=MAX_EXPIRY_BLOCKS).contains(&blocks) {
        return Err(CheckoutError::BadRequest(format!(
            "expires_in_blocks must be {MIN_EXPIRY_BLOCKS} to {MAX_EXPIRY_BLOCKS}"
        )));
    }
    Ok(())
}

/// A fresh invoice id: 128 random bits from the OS, hex.
///
/// # Errors
///
/// [`CheckoutError::Internal`] if the OS random source fails.
pub fn new_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| CheckoutError::Internal(format!("randomness: {e}")))?;
    Ok(hex::encode(bytes))
}

/// Base-unit amounts as decimal strings (no sign, no leading zeros).
pub mod amount {
    use primitive_types::U256;
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::error::{CheckoutError, Result};

    /// Parses a canonical decimal amount.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::BadRequest`] for anything but digits, a leading zero,
    /// or a value above 256 bits.
    pub fn parse(text: &str) -> Result<U256> {
        let canonical = !text.is_empty()
            && text.bytes().all(|b| b.is_ascii_digit())
            && (text == "0" || !text.starts_with('0'));
        if !canonical {
            return Err(CheckoutError::BadRequest(
                "amount must be decimal base units with no leading zeros".into(),
            ));
        }
        U256::from_dec_str(text)
            .map_err(|_| CheckoutError::BadRequest("amount is too large".into()))
    }

    /// Serde adapter: writes the amount as a decimal string.
    pub fn serialize<S: Serializer>(value: &U256, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }

    /// Serde adapter: reads a decimal string.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<U256, D::Error> {
        let text = String::deserialize(d)?;
        parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Serde adapter for 32-byte addresses, written as hex.
mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8; 32], s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<[u8; 32], D::Error> {
        let text = String::deserialize(d)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("address is not 32 bytes"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn amount_parse_accepts_canonical_and_rejects_the_rest() {
        assert_eq!(amount::parse("0").unwrap(), U256::zero());
        assert_eq!(amount::parse("1000").unwrap(), U256::from(1000u64));
        for bad in ["", "01", "-1", "1.5", "0x10", " 1", "1e3"] {
            assert!(amount::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn memo_rules() {
        assert!(validate_memo("order-42").is_ok());
        assert!(validate_memo(&"a".repeat(MAX_MEMO_BYTES + 1)).is_err());
        assert!(validate_memo("bad\nmemo").is_err());
    }

    #[test]
    fn expiry_bounds() {
        assert!(validate_expiry(0).is_err());
        assert!(validate_expiry(MIN_EXPIRY_BLOCKS).is_ok());
        assert!(validate_expiry(MAX_EXPIRY_BLOCKS + 1).is_err());
    }

    #[test]
    fn open_rejects_zero_amount() {
        let req = NewInvoice {
            amount: U256::zero(),
            memo: "x".into(),
            expires_in_blocks: 10,
            webhook_url: "https://shop.example/hook".into(),
        };
        assert!(Invoice::open("id".into(), [0; 32], &req, 5).is_err());
    }
}
