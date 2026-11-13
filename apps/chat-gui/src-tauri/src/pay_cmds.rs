//! In-chat wallet actions: request, pay and receipt bubbles (ADR-088,
//! ADR-138). Chat signs only its own messages with the chat identity; money
//! moves only when the user confirms in the wallet, which holds the key.
//! Requests and receipts name wallet accounts through a binding the wallet
//! signed (ADR-088 amendment 1), set once with [`pay_settings`].

use std::sync::Arc;

use maya_chat::client::Frame;
use maya_chat::miniapp_bridge::pay_confirm_text;
use maya_chat::pay::binding::Binding;
use maya_chat::pay::chain::{HttpRpc, tip_height};
use maya_chat::pay::ledger::Due;
use maya_chat::pay::{Paid, Request, Terms};
use maya_chat::pay_link::{Network, base_to_whole, parse_paid_uri, pay_uri, whole_to_base};
use maya_chat::{courier, now};
use tauri::State;

use crate::store::{Line, PayBubble};
use crate::{App, address, relay_addr, remember, why};

/// shortcut: testnet only, as the default relay is; follow the relay's chain
/// once a mainnet chat relay exists.
pub(crate) const NETWORK: Network = Network::Testnet;
/// How long a request's Pay button stays live. Policy, within pay::MAX_TTL.
const REQUEST_TTL: u64 = 3_600;

fn line(peer: [u8; 32], mine: bool, text: String, pay: PayBubble) -> Line {
    Line {
        peer: hex::encode(peer),
        mine,
        text,
        at: now(),
        sticker: None,
        outbox: None,
        pay: Some(pay),
    }
}

/// The bubble for a payment frame, or `None` for any other frame.
pub(crate) fn bubble(_me: &[u8; 32], frame: Frame) -> Option<Line> {
    match frame {
        Frame::Pay { from, id, request } => {
            let amount = base_to_whole(&request.amount);
            let asker = format!("Chat contact {}", hex::encode(from));
            let pay = PayBubble {
                kind: "request".into(),
                amount: amount.clone(),
                memo: request.memo.clone(),
                request: hex::encode(id),
                link: Some(pay_uri(NETWORK, &Due::of(&request), &id)),
                // The account paid is the contact's bound wallet, never its chat id.
                confirm: Some(pay_confirm_text(
                    &asker,
                    NETWORK,
                    &request.pay_to,
                    &request.amount,
                    &request.memo,
                )),
                tx: None,
            };
            Some(line(
                from,
                false,
                format!("Payment request: {amount} coins"),
                pay,
            ))
        }
        Frame::Paid {
            from,
            request_id,
            tx_id,
        } => {
            let pay = PayBubble {
                kind: "paid".into(),
                amount: String::new(),
                memo: String::new(),
                request: hex::encode(request_id),
                link: None,
                confirm: None,
                tx: Some(hex::encode(tx_id)),
            };
            // Not settled: the chain check (pay::chain::find_final, Client::settle) is not wired here yet.
            Some(line(
                from,
                false,
                "Says it paid (not yet checked on chain)".into(),
                pay,
            ))
        }
        Frame::Text(_) | Frame::Receipt { .. } | Frame::Binding { .. } => None,
    }
}

/// Sets what payments need: the chain's genesis id (hex), the node that
/// reports its tip (a request records the tip it was issued at), and the
/// binding the wallet's "Chat payment binding" produced (hex). A revocation
/// is accepted too: it clears the binding, and is sent to every contact.
#[tauri::command]
pub(crate) async fn pay_settings(
    app: State<'_, Arc<App>>,
    chain: String,
    node: String,
    binding: String,
) -> Result<(), String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let chain = address(chain.trim()).map_err(|_| "the chain id must be 64 hex characters")?;
    let bytes = hex::decode(binding.trim()).map_err(|_| "the binding is not hex")?;
    let mut rest = bytes.as_slice();
    let binding = Binding::take(&mut rest).map_err(|e| why(&e))?;
    if !rest.is_empty() {
        return Err("the binding has extra bytes".into());
    }
    let at = now();
    s.client.set_pay_chain(chain);
    s.client
        .set_pay_binding(binding.clone(), at)
        .map_err(|e| why(&e))?;
    s.profile.pay_node = Some(node.trim().to_owned());
    if binding.kind == maya_chat::pay::BindingKind::Revoke {
        let version = s.client.send_version();
        for contact in s.profile.contacts.clone() {
            let to = address(&contact.address)?;
            if s.client.has_session(&to) {
                s.client
                    .queue_binding(to, &binding, at, version)
                    .map_err(|e| why(&e))?;
            }
        }
        app.save(s)?;
        let relay = relay_addr(&s.profile)?;
        let _attempt = courier::flush(&mut s.client, &relay, now()).await;
    }
    app.save(s)
}

/// Asks `peer` for `amount` whole coins, to this identity's bound wallet.
/// The peer must already have a session with us: a request cannot open one
/// (ADR-088).
#[tauri::command]
pub(crate) async fn pay_request(
    app: State<'_, Arc<App>>,
    peer: String,
    amount: String,
    memo: String,
) -> Result<Line, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let to = address(&peer)?;
    if !s.client.has_session(&to) {
        return Err("send them a message first; a request cannot start a chat".into());
    }
    let (Some(chain), Some(binding)) = (s.client.pay_chain(), s.client.pay_binding().cloned())
    else {
        return Err("set up payments first: chain, node and your wallet binding".into());
    };
    let node = s
        .profile
        .pay_node
        .clone()
        .ok_or("set the node in payment settings")?;
    let tip = tip_height(&HttpRpc::new(node), &chain)
        .await
        .map_err(|e| why(&e))?;
    let base = whole_to_base(amount.trim()).map_err(|e| why(&e))?;
    let at = now();
    let terms = Terms {
        chain,
        binding: &binding,
        amount: &base,
        memo: &memo,
        expires_at: at + REQUEST_TTL,
    };
    let request = Request::sign(s.client.identity(), &to, &terms, at).map_err(|e| why(&e))?;
    let version = s.client.send_version();
    s.client
        .queue_request(to, &request, tip, at, version)
        .map_err(|e| why(&e))?;
    let relay = relay_addr(&s.profile)?;
    app.save(s)?;
    // A failed flush leaves the request queued; `poll` retries it.
    let _attempt = courier::flush(&mut s.client, &relay, now()).await;
    let pay = PayBubble {
        kind: "request".into(),
        amount: base_to_whole(&base),
        memo,
        request: hex::encode(request.id(&to)),
        link: None,
        confirm: None,
        tx: None,
    };
    let line = line(to, true, format!("You asked for {} coins", pay.amount), pay);
    remember(&mut s.profile, line.clone());
    app.save(s)?;
    Ok(line)
}

/// Tells `peer` (the payee) that we paid, from the wallet's return link
/// (`maya2c-chat://paid?request=…&tx=…`). The receipt carries our wallet
/// binding: the payee settles only on a payment from that account.
#[tauri::command]
pub(crate) async fn pay_receipt(
    app: State<'_, Arc<App>>,
    peer: String,
    paid_link: String,
) -> Result<Line, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let to = address(&peer)?;
    let link = parse_paid_uri(paid_link.trim()).map_err(|e| why(&e))?;
    let (Some(chain), Some(binding)) = (s.client.pay_chain(), s.client.pay_binding().cloned())
    else {
        return Err("set up payments first: chain, node and your wallet binding".into());
    };
    let paid = Paid::sign(
        s.client.identity(),
        &binding,
        &to,
        &chain,
        link.request_id,
        link.tx_id,
    )
    .map_err(|e| why(&e))?;
    let version = s.client.send_version();
    s.client
        .queue_paid(to, &paid, now(), version)
        .map_err(|e| why(&e))?;
    let relay = relay_addr(&s.profile)?;
    app.save(s)?;
    let _attempt = courier::flush(&mut s.client, &relay, now()).await;
    let pay = PayBubble {
        kind: "receipt".into(),
        amount: String::new(),
        memo: String::new(),
        request: hex::encode(link.request_id),
        link: None,
        confirm: None,
        tx: Some(hex::encode(link.tx_id)),
    };
    let line = line(to, true, "You said you paid".into(), pay);
    remember(&mut s.profile, line.clone());
    app.save(s)?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maya_chat::pay::BindingKind;
    use maya_chat::pay_link::parse_pay_uri;

    const CHAIN: [u8; 32] = [0xc1; 32];
    const WALLET: [u8; 32] = [0xaa; 32];

    /// A received request as `Client::take_pay_frames` hands it over, after
    /// its checks. The bubble only renders it, so no signature is needed.
    fn received() -> (Frame, [u8; 32]) {
        let payee = [1; 32];
        let binding = Binding {
            kind: BindingKind::Bind,
            chain: CHAIN,
            chat_id: payee,
            account: WALLET,
            issued_at: 1_000,
            expires_at: 4_600,
            public_key: Vec::new(),
            signature: Vec::new(),
        };
        let request = Request {
            payee,
            pay_to: WALLET,
            binding,
            chain: CHAIN,
            amount: "2500000000000000000000000".into(),
            memo: "lunch".into(),
            expires_at: 2_000,
            nonce: [3; 16],
            signature: Vec::new(),
        };
        let id = [4; 32];
        let frame = Frame::Pay {
            from: payee,
            id,
            request: Box::new(request),
        };
        (frame, id)
    }

    #[test]
    fn a_received_request_links_to_the_bound_wallet_on_its_chain() {
        let (frame, id) = received();
        let line = bubble(&[2; 32], frame).unwrap();
        let pay = line.pay.unwrap();
        assert_eq!(pay.amount, "2.5");
        assert!(pay.confirm.unwrap().contains("Amount: 2.5 coins"));
        let link = parse_pay_uri(&pay.link.unwrap(), NETWORK).unwrap();
        assert_eq!(link.to, WALLET, "the wallet account, not the chat id");
        assert_eq!((link.chain, link.request_id), (CHAIN, id));
    }
}
