//! Payment requests, QR decoding, and offline signing.
//!
//! ## QR payloads are parsed in Rust
//!
//! A scanned QR code is attacker-controlled input: anyone can print one. Parsing
//! it in the frontend would put that string inside the trust boundary, where a
//! malformed or hostile payload is one bug away from influencing what gets
//! signed. Decoding and parsing both happen here, and the frontend receives a
//! validated [`PaymentRequest`] or an error — never the raw text.
//!
//! ## Offline signing
//!
//! [`sign_transfer`] needs no network. It produces the raw hex a node accepts,
//! which can be moved to a connected machine by any means — including a QR code
//! displayed on an air-gapped device. Broadcasting is a separate, explicit step
//! so the signing machine never has to touch a network.

use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use serde::{Deserialize, Serialize};

use crate::error::{Result, WalletError};

/// URI scheme for payment requests.
pub const URI_SCHEME: &str = "maya:";

/// Largest QR image accepted, in pixels.
///
/// A camera frame is bounded; anything larger is not a frame and decoding it
/// would be an easy way to make the wallet spend unbounded time.
pub const MAX_QR_PIXELS: u32 = 4096;

/// A request to pay someone.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaymentRequest {
    /// Hex-encoded recipient address.
    pub recipient: String,
    /// Amount in base units, if the request specifies one.
    pub amount: Option<u64>,
    /// Free-text label supplied by the requester.
    ///
    /// Untrusted and unauthenticated — anyone can put any name here. The UI
    /// must present it as a claim, never as an identity.
    pub label: Option<String>,
}

impl PaymentRequest {
    /// Parses a `maya:` URI.
    ///
    /// Accepted forms:
    ///
    /// ```text
    /// maya:<64-hex-address>
    /// maya:<64-hex-address>?amount=1000
    /// maya:<64-hex-address>?amount=1000&label=Coffee
    /// ```
    ///
    /// A bare 64-character hex address is also accepted, since that is what
    /// most people paste.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError::Qr`] for a malformed URI, an address that is not
    /// 32 bytes of hex, or an unparseable amount.
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        let body = trimmed.strip_prefix(URI_SCHEME).unwrap_or(trimmed);

        let (address, query) = match body.split_once('?') {
            Some((address, query)) => (address, Some(query)),
            None => (body, None),
        };

        let recipient = normalize_address(address)?;

        let mut amount = None;
        let mut label = None;

        if let Some(query) = query {
            for pair in query.split('&').filter(|p| !p.is_empty()) {
                let (key, value) = pair.split_once('=').ok_or_else(|| {
                    WalletError::Qr(format!("malformed query parameter '{pair}'"))
                })?;
                match key {
                    "amount" => {
                        amount = Some(value.parse::<u64>().map_err(|_| {
                            WalletError::Qr(format!("amount '{value}' is not a number"))
                        })?);
                    }
                    "label" => label = Some(percent_decode(value)),
                    // Unknown parameters are ignored rather than rejected, so a
                    // newer wallet's extra fields do not break an older one.
                    _ => {}
                }
            }
        }

        Ok(Self {
            recipient,
            amount,
            label,
        })
    }

    /// Renders this request as a `maya:` URI.
    #[must_use]
    pub fn to_uri(&self) -> String {
        let mut uri = format!("{URI_SCHEME}{}", self.recipient);
        let mut separator = '?';
        if let Some(amount) = self.amount {
            uri.push(separator);
            uri.push_str(&format!("amount={amount}"));
            separator = '&';
        }
        if let Some(label) = &self.label {
            uri.push(separator);
            uri.push_str(&format!("label={}", percent_encode(label)));
        }
        uri
    }
}

/// Validates and lowercases a hex address.
///
/// # Errors
///
/// Returns [`WalletError::Address`] unless the input is exactly 32 bytes of hex.
pub fn normalize_address(address: &str) -> Result<String> {
    let cleaned = address.trim().trim_start_matches("0x");
    let bytes = hex::decode(cleaned)
        .map_err(|_| WalletError::Address(format!("'{address}' is not hexadecimal")))?;
    if bytes.len() != 32 {
        return Err(WalletError::Address(format!(
            "an address is 32 bytes; '{address}' decodes to {}",
            bytes.len()
        )));
    }
    Ok(hex::encode(bytes))
}

/// Decodes a `[u8; 32]` address.
///
/// # Errors
///
/// As [`normalize_address`].
pub fn decode_address(address: &str) -> Result<[u8; 32]> {
    let normalized = normalize_address(address)?;
    let bytes = hex::decode(normalized).map_err(|e| WalletError::Address(e.to_string()))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Minimal percent-decoding for label values.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex_pair = core::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex_pair, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        if bytes[index] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[index]);
        }
        index += 1;
    }

    // Invalid UTF-8 becomes replacement characters rather than an error: a bad
    // label should not stop a user reading the address they are paying.
    String::from_utf8_lossy(&out).into_owned()
}

/// Minimal percent-encoding for label values.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Decodes the first QR code in a PNG image.
///
/// # Errors
///
/// Returns [`WalletError::Qr`] if the image cannot be decoded, is larger than
/// [`MAX_QR_PIXELS`] on a side, or contains no readable QR code.
pub fn decode_qr_png(png: &[u8]) -> Result<String> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| WalletError::Qr(format!("not a readable PNG: {e}")))?;

    if image.width() > MAX_QR_PIXELS || image.height() > MAX_QR_PIXELS {
        return Err(WalletError::Qr(format!(
            "image is {}x{}, larger than the {MAX_QR_PIXELS} pixel limit",
            image.width(),
            image.height()
        )));
    }

    let luma = image.to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(luma);
    let grids = prepared.detect_grids();

    for grid in grids {
        if let Ok((_meta, content)) = grid.decode() {
            return Ok(content);
        }
    }

    Err(WalletError::Qr("no QR code found in the image".to_string()))
}

/// Scans a QR image and parses it as a payment request.
///
/// # Errors
///
/// Propagates decode and parse failures.
pub fn scan_payment_request(png: &[u8]) -> Result<PaymentRequest> {
    PaymentRequest::parse(&decode_qr_png(png)?)
}

/// A transaction signed and ready to broadcast.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedTransfer {
    /// Hex-encoded transaction id.
    pub txid: String,
    /// Hex-encoded sender address.
    pub sender: String,
    /// Hex-encoded recipient address.
    pub recipient: String,
    /// Amount transferred.
    pub amount: u64,
    /// Sender nonce used.
    pub nonce: u64,
    /// Fee attached.
    pub fee: u64,
    /// The raw transaction, hex encoded, as a node accepts it.
    pub raw_hex: String,
}

/// Signs a transfer without touching the network.
///
/// The `fee` is carried as an explicit second output to a burn address, which
/// is what gives the user manual fee control: the chain has no separate fee
/// field, so a fee is value the sender deliberately does not send to the
/// recipient.
///
/// # Errors
///
/// Returns [`WalletError::Address`] for a malformed recipient, or
/// [`WalletError::AmountOverflow`] if amount and fee overflow together.
pub fn sign_transfer(
    signing_key: &HybridSigningKey,
    recipient: &str,
    amount: u64,
    fee: u64,
    nonce: u64,
) -> Result<SignedTransfer> {
    sign_with_fee_to(signing_key, recipient, amount, fee, FEE_SINK, nonce)
}

/// Signs a transfer on a fee-market chain (ADR-029). No network access.
///
/// The fee is an ordinary signed output to `fee_recipient`, the collector the
/// node names in `get_fee_info` — not the burn sink, which such a node
/// refuses as payment. The chain burns the base-fee part itself; anything
/// above it is a tip to validators. Fetch the collector while online and sign
/// here, so an air-gapped device can still sign.
///
/// # Errors
///
/// [`WalletError::Address`] for a malformed recipient or collector, or
/// [`WalletError::AmountOverflow`] if amount and fee overflow together.
pub fn sign_transfer_to(
    signing_key: &HybridSigningKey,
    recipient: &str,
    amount: u64,
    fee: u64,
    fee_recipient: &str,
    nonce: u64,
) -> Result<SignedTransfer> {
    let fee_recipient = decode_address(fee_recipient)?;
    sign_with_fee_to(signing_key, recipient, amount, fee, fee_recipient, nonce)
}

/// Refuses any fee collector but the chain's own.
///
/// The wallet learns the collector from whatever node or gateway it is
/// pointed at. The real one is a derived address fixed in the node, which
/// nobody holds a key for, so a node that names another address is asking
/// for the fee to be paid to itself. Checked here rather than trusted.
///
/// # Errors
///
/// [`WalletError::Address`] if `collector` is malformed or is not the chain's
/// fee collector.
pub fn check_fee_collector(collector: &str) -> Result<()> {
    if decode_address(collector)? == custom_l1_node::state::fees::FEE_COLLECTOR {
        Ok(())
    } else {
        Err(WalletError::Address(format!(
            "{collector} is not this chain's fee collector; refusing to pay a fee to it"
        )))
    }
}

/// Serialized size of a transfer that carries a fee output, in bytes.
///
/// What a fee-market chain charges `base_fee` per byte for. Output encoding
/// does not depend on amounts or on which recipient, so a probe signed with
/// placeholder values has exactly the size of the real transfer.
///
/// # Errors
///
/// [`WalletError::Address`] for a malformed collector; signing failures.
pub fn transfer_size(
    signing_key: &HybridSigningKey,
    fee_recipient: &str,
    nonce: u64,
) -> Result<u64> {
    let probe = sign_transfer_to(signing_key, fee_recipient, 1, 1, fee_recipient, nonce)?;
    u64::try_from(probe.raw_hex.len() / 2).map_err(|_| WalletError::AmountOverflow)
}

/// Economy, Standard and Priority fees for a transfer of `size` bytes.
///
/// Economy is exactly `base_fee x size`: accepted now, refused if the base
/// fee rises before inclusion. Standard is twice that, the command-line
/// wallet's default; Priority is four times.
///
/// # Errors
///
/// [`WalletError::AmountOverflow`] if a fee does not fit in a `u64`.
pub fn priced_fee_tiers(base_fee: u64, size: u64) -> Result<[u64; 3]> {
    let required = base_fee
        .checked_mul(size)
        .ok_or(WalletError::AmountOverflow)?;
    let tier = |factor: u64| {
        required
            .checked_mul(factor)
            .ok_or(WalletError::AmountOverflow)
    };
    Ok([required, tier(2)?, tier(4)?])
}

fn sign_with_fee_to(
    signing_key: &HybridSigningKey,
    recipient: &str,
    amount: u64,
    fee: u64,
    fee_recipient: [u8; 32],
    nonce: u64,
) -> Result<SignedTransfer> {
    let recipient_bytes = decode_address(recipient)?;

    // Checked, because a wrapped total would sign a transaction the sender
    // never agreed to.
    amount.checked_add(fee).ok_or(WalletError::AmountOverflow)?;

    let mut outputs = vec![TxOutput {
        amount,
        recipient: recipient_bytes,
    }];
    if fee > 0 {
        outputs.push(TxOutput {
            amount: fee,
            recipient: fee_recipient,
        });
    }

    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(signing_key)
        .map_err(|e| WalletError::Signing(e.to_string()))?;

    Ok(SignedTransfer {
        txid: hex::encode(tx.txid()),
        // The sender's address, not the 1952-byte key it hashes from. A UI
        // field holding 3904 hex characters is not an address anybody can read
        // back to a counterparty.
        sender: hex::encode(tx.sender()),
        recipient: normalize_address(recipient)?,
        amount,
        nonce,
        fee,
        raw_hex: hex::encode(tx.to_bytes()),
    })
}

/// Address fees are paid to.
///
/// All zeros: an unspendable address, because no private key produces the
/// identity point as a public key. Fees are burned rather than paid to a miner,
/// since the chain has no coinbase mechanism to credit one.
pub const FEE_SINK: [u8; 32] = [0u8; 32];

/// Fee presets offered in the UI.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FeeTier {
    /// Cheapest, slowest.
    Economy,
    /// Balanced default.
    Standard,
    /// Highest, for time-sensitive payments.
    Priority,
}

impl FeeTier {
    /// The fee this tier suggests, in base units.
    #[must_use]
    pub fn suggested_fee(self) -> u64 {
        match self {
            Self::Economy => 1,
            Self::Standard => 10,
            Self::Priority => 100,
        }
    }

    /// Every tier, for rendering a selector.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Economy, Self::Standard, Self::Priority]
    }

    /// Display label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Economy => "Economy",
            Self::Standard => "Standard",
            Self::Priority => "Priority",
        }
    }
}
