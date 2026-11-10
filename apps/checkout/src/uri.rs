//! The payment link and its QR code.
//!
//! Format: `maya:<bech32m address>?amount=<base units>&id=<percent-encoded order id>`.
//! This is the payment request v2 form of ADR-086 (`wallet-v2`), which is
//! `maya:<bech32m>?amount=&label=&message=&exp=&id=`. The checkout emits the
//! recipient as Bech32m and the order id as `id`. `exp` and `message` are not
//! emitted yet: their units are not fixed here. The v1 parser in
//! `apps/wallet-gui/core/src/payment.rs` reads `amount` and ignores `id`, so
//! an older wallet still pays the right address and amount. ADR-091 records
//! why no separate `maya2c-wallet://` scheme is used.

use maya_address::Network;
use primitive_types::U256;

use crate::error::{CheckoutError, Result};

/// Scheme prefix the wallet's payment parser strips.
pub const URI_SCHEME: &str = "maya:";

/// Renders a payment link for `recipient` on `network`. `order_id` is the
/// shop's order reference, carried as the `id` parameter.
#[must_use]
pub fn payment_uri(network: Network, recipient: &[u8; 32], amount: U256, order_id: &str) -> String {
    format!(
        "{URI_SCHEME}{}?amount={amount}&id={}",
        maya_address::encode(network, recipient),
        percent_encode(order_id)
    )
}

/// The QR code for `uri` as an SVG document, dark modules on white.
///
/// White background: phone cameras expect dark-on-light, and an inverted code
/// fails on many of them. Medium error correction (15%) tolerates a glare spot.
///
/// # Errors
///
/// [`CheckoutError::Internal`] if the URI does not fit in a QR code.
pub fn payment_qr_svg(uri: &str) -> Result<String> {
    let code = qrcode::QrCode::with_error_correction_level(uri, qrcode::EcLevel::M)
        .map_err(|e| CheckoutError::Internal(format!("qr encode: {e}")))?;
    Ok(code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#05070f"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

/// Percent-encodes everything outside the RFC 3986 unreserved set.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn uri_matches_the_wallet_format() {
        let uri = payment_uri(Network::Testnet, &[7u8; 32], U256::from(1500u64), "order 42&x");
        assert!(uri.starts_with("maya:tmaya1"), "{uri}");
        assert!(uri.ends_with("?amount=1500&id=order%2042%26x"), "{uri}");
    }

    #[test]
    fn qr_svg_renders() {
        let svg = payment_qr_svg("maya:tmaya1example?amount=1").unwrap();
        assert!(svg.contains("<svg"));
    }
}
