//! What `sign_transfer` refuses before anyone is asked to approve a payment
//! (ADR-086 §7, ADR-088). Pure, so every refusal is tested without a window,
//! and `sign_transfer` cannot drop one without a test failing.
//!
//! A chat request is taken from what the wallet itself handed the UI from a
//! link ([`Opened`]), never from what the UI sends back: a webview that
//! leaves the request out still gets the already-paid check.

use maya_wallet_core::chat_pay::{self, PaidRequests};
use maya_wallet_core::payment::{ChatRequest, decode_address};
use maya_wallet_core::wallet;
use primitive_types::U256;

/// Chat requests remembered per run, newest last. A person works through a
/// handful of links at a time; the oldest beyond this is dropped.
pub const MAX_OPENED: usize = 16;

/// A chat request the UI was handed from a link, and the account it pays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The request id and its chain, as the link carried them.
    pub chat: ChatRequest,
    /// The 32-byte account the link pays.
    pub recipient: [u8; 32],
}

/// Remembers `entry`, replacing an earlier one with the same request id.
pub fn remember(opened: &mut Vec<Opened>, entry: Opened) {
    opened.retain(|o| o.chat.request != entry.chat.request);
    if opened.len() >= MAX_OPENED {
        opened.remove(0);
    }
    opened.push(entry);
}

/// The chat request a transfer to `recipient` pays. `given` names one by
/// id, and must be one opened for this recipient. Without it, the newest
/// one opened for `recipient` applies, so leaving it out skips nothing.
///
/// # Errors
///
/// `given` names a request never opened from a link, or opened for another
/// recipient.
pub fn resolve_chat(
    opened: &[Opened],
    given: Option<&ChatRequest>,
    recipient: &[u8; 32],
) -> Result<Option<ChatRequest>, String> {
    let for_recipient = opened.iter().filter(|o| &o.recipient == recipient);
    match given {
        Some(given) => for_recipient
            .clone()
            .find(|o| o.chat.request == given.request)
            .map(|o| Some(o.chat.clone()))
            .ok_or_else(|| {
                "this payment request was not opened from a link to this recipient; nothing was signed"
                    .to_string()
            }),
        None => Ok(for_recipient.last().map(|o| o.chat.clone())),
    }
}

/// A recipient as `sign_transfer` receives it, Bech32m or hex, as 32 bytes.
///
/// # Errors
///
/// Neither a known-network address nor 32 bytes of hex.
pub fn recipient_id(text: &str) -> Result<[u8; 32], String> {
    let compact: String = text.split_whitespace().collect();
    maya_address::decode_any(&compact)
        .map(|(_, id)| id)
        .or_else(|_| decode_address(&compact))
        .map_err(|_| "the recipient is not an address; nothing was signed".to_string())
}

/// Everything checked before the native confirm screen.
pub struct PreSign<'a> {
    /// The request's `exp`, if it has one.
    pub expires_at: Option<u64>,
    /// This machine's clock, unix seconds; `None` if it cannot be read.
    pub now: Option<u64>,
    /// The chat request paid, from [`resolve_chat`].
    pub chat: Option<&'a ChatRequest>,
    /// The node's genesis id, hex.
    pub genesis: &'a str,
    /// The paid list, read from disk.
    pub paid: &'a PaidRequests,
    /// The fee offered.
    pub fee: U256,
    /// The amount sent.
    pub amount: U256,
}

/// Runs the refusals in order: expiry, the chat request's chain and paid
/// list (with room to record it), the fee cap. Returns the chat request id
/// to mark paid.
///
/// # Errors
///
/// The refusal, worded for the person.
pub fn before_dialog(c: &PreSign<'_>) -> Result<Option<[u8; 32]>, String> {
    if let Some(exp) = c.expires_at {
        // An unreadable clock refuses: it cannot show the request is live.
        let now = c.now.ok_or(
            "this computer's clock cannot be read, so the request's expiry cannot be checked; nothing was signed",
        )?;
        if now > exp {
            return Err("the payment request has expired; nothing was signed".to_string());
        }
    }
    let id = c
        .chat
        .map(|chat| check_chat(chat, c.genesis, c.paid, c.now))
        .transpose()?;
    // A person is never asked to approve a fee the core would not sign.
    wallet::check_fee_cap(c.fee, c.amount, false).map_err(|e| e.to_string())?;
    Ok(id)
}

fn check_chat(
    chat: &ChatRequest,
    genesis: &str,
    paid: &PaidRequests,
    now: Option<u64>,
) -> Result<[u8; 32], String> {
    let link_chain = hex32(&chat.chain).ok_or("the request's chain id is malformed")?;
    let id = hex32(&chat.request).ok_or("the request id is malformed")?;
    let genesis = hex32(genesis).ok_or("the node's genesis id is malformed")?;
    chat_pay::check_pay_chain(&link_chain, &genesis).map_err(|e| e.to_string())?;
    let now = now.ok_or("this computer's clock cannot be read; nothing was signed")?;
    paid.check_room(&id, now).map_err(|e| e.to_string())?;
    Ok(id)
}

/// 32 bytes of hex.
pub fn hex32(text: &str) -> Option<[u8; 32]> {
    hex::decode(text).ok()?.try_into().ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;
    use maya_wallet_core::chat_pay::MAX_PAID;

    const GENESIS: [u8; 32] = [0xc1; 32];
    const TO: [u8; 32] = [7; 32];
    const NOW: u64 = 1_000_000;

    fn chat(id: u8, chain: [u8; 32]) -> ChatRequest {
        ChatRequest {
            request: hex::encode([id; 32]),
            chain: hex::encode(chain),
        }
    }

    fn checks<'a>(chat: Option<&'a ChatRequest>, paid: &'a PaidRequests) -> PreSign<'a> {
        PreSign {
            expires_at: None,
            now: Some(NOW),
            chat,
            genesis: "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1",
            paid,
            fee: U256::from(1_000),
            amount: U256::from(1_000_000),
        }
    }

    #[test]
    fn a_fresh_chat_request_passes_and_names_its_id() {
        let paid = PaidRequests::default();
        let request = chat(1, GENESIS);
        assert_eq!(
            before_dialog(&checks(Some(&request), &paid)),
            Ok(Some([1; 32]))
        );
        assert_eq!(before_dialog(&checks(None, &paid)), Ok(None));
    }

    #[test]
    fn a_paid_request_is_refused_before_the_dialog() {
        let mut paid = PaidRequests::default();
        paid.record(&[1; 32], NOW).unwrap();
        let error = before_dialog(&checks(Some(&chat(1, GENESIS)), &paid)).unwrap_err();
        assert!(error.contains("already paid"), "{error}");
    }

    #[test]
    fn a_request_for_another_chain_is_refused() {
        let paid = PaidRequests::default();
        let error = before_dialog(&checks(Some(&chat(1, [0xc2; 32])), &paid)).unwrap_err();
        assert!(error.contains("another chain"), "{error}");
    }

    #[test]
    fn a_full_paid_list_is_refused_before_the_dialog() {
        let mut paid = PaidRequests::default();
        for n in 0..MAX_PAID {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&(n as u64).to_le_bytes());
            paid.record(&id, NOW).unwrap();
        }
        let error = before_dialog(&checks(Some(&chat(0xff, GENESIS)), &paid)).unwrap_err();
        assert!(error.contains("too many"), "{error}");
    }

    #[test]
    fn expiry_and_the_fee_cap_still_refuse() {
        let paid = PaidRequests::default();
        let mut expired = checks(None, &paid);
        expired.expires_at = Some(NOW - 1);
        assert!(before_dialog(&expired).unwrap_err().contains("expired"));
        let mut no_clock = checks(None, &paid);
        no_clock.expires_at = Some(NOW + 1);
        no_clock.now = None;
        assert!(before_dialog(&no_clock).unwrap_err().contains("clock"));
        let mut greedy = checks(None, &paid);
        greedy.fee = greedy.amount * 10;
        assert!(before_dialog(&greedy).is_err(), "fee cap");
    }

    #[test]
    fn a_chat_request_comes_from_the_link_not_the_webview() {
        let mut opened = Vec::new();
        remember(
            &mut opened,
            Opened {
                chat: chat(1, GENESIS),
                recipient: TO,
            },
        );
        // Left out by the UI: still applied to a transfer to that recipient.
        assert_eq!(resolve_chat(&opened, None, &TO), Ok(Some(chat(1, GENESIS))));
        assert_eq!(resolve_chat(&opened, None, &[8; 32]), Ok(None));
        // The chain is the link's, whatever the UI claims.
        assert_eq!(
            resolve_chat(&opened, Some(&chat(1, [0xc2; 32])), &TO),
            Ok(Some(chat(1, GENESIS)))
        );
        // Never opened, or opened for someone else: refused.
        assert!(resolve_chat(&opened, Some(&chat(2, GENESIS)), &TO).is_err());
        assert!(resolve_chat(&opened, Some(&chat(1, GENESIS)), &[8; 32]).is_err());
    }

    #[test]
    fn remembered_requests_are_bounded() {
        let mut opened = Vec::new();
        for id in 0..=u8::try_from(MAX_OPENED).unwrap() {
            remember(
                &mut opened,
                Opened {
                    chat: chat(id, GENESIS),
                    recipient: TO,
                },
            );
        }
        assert_eq!(opened.len(), MAX_OPENED);
        assert_eq!(opened[0].chat, chat(1, GENESIS), "oldest dropped");
    }

    #[test]
    fn recipients_decode_from_either_spelling() {
        let bech = maya_address::encode(maya_address::Network::Testnet, &TO);
        assert_eq!(recipient_id(&bech), Ok(TO));
        assert_eq!(recipient_id(&hex::encode(TO)), Ok(TO));
        assert!(recipient_id("not an address").is_err());
    }
}
