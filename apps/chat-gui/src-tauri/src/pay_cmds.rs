//! In-chat wallet actions: request, pay and receipt bubbles (ADR-088,
//! ADR-138). Chat signs only its own messages with the chat identity; money
//! moves only when the user confirms in the wallet, which holds the key.

use std::sync::Arc;

use maya_chat::client::Frame;
use maya_chat::miniapp_bridge::pay_confirm_text;
use maya_chat::pay::{Paid, Request};
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
pub(crate) fn bubble(me: &[u8; 32], frame: Frame) -> Option<Line> {
    match frame {
        Frame::Pay { from, id, request } => {
            let amount = base_to_whole(&request.amount);
            let asker = format!("Chat contact {}", hex::encode(from));
            let pay = PayBubble {
                kind: "request".into(),
                amount: amount.clone(),
                memo: request.memo.clone(),
                request: hex::encode(id),
                link: Some(pay_uri(NETWORK, &request, me)),
                confirm: Some(pay_confirm_text(
                    &asker,
                    NETWORK,
                    &request.payee,
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
            // Not settled: the chain check (pay::settles_on_chain) is not wired here yet.
            Some(line(
                from,
                false,
                "Says it paid (not yet checked on chain)".into(),
                pay,
            ))
        }
        Frame::Text(_) | Frame::Receipt { .. } => None,
    }
}

/// Asks `peer` for `amount` whole coins. The peer must already have a
/// session with us: a request cannot open one (ADR-088).
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
    let base = whole_to_base(amount.trim()).map_err(|e| why(&e))?;
    let at = now();
    let request = Request::sign(s.client.identity(), &to, &base, &memo, at + REQUEST_TTL, at)
        .map_err(|e| why(&e))?;
    let version = s.client.send_version();
    s.client
        .queue_request(to, &request, at, version)
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
/// (`maya2c-chat://paid?request=…&tx=…`).
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
    let paid =
        Paid::sign(s.client.identity(), &to, link.request_id, link.tx_id).map_err(|e| why(&e))?;
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
    use maya_chat::identity::Identity;

    #[test]
    fn a_received_request_carries_its_confirm_text_and_link() {
        let payee = Identity::from_seed([1; 32]);
        let me = Identity::from_seed([2; 32]).address();
        let request = Request::sign(
            &payee,
            &me,
            "2500000000000000000000000",
            "lunch",
            2_000,
            1_000,
        )
        .unwrap();
        let id = request.id(&me);
        let frame = Frame::Pay {
            from: payee.address(),
            id,
            request,
        };
        let line = bubble(&me, frame).unwrap();
        let pay = line.pay.unwrap();
        assert_eq!(pay.amount, "2.5");
        assert!(pay.confirm.unwrap().contains("Amount: 2.5 coins"));
        assert!(pay.link.unwrap().starts_with("maya2c-wallet://pay?"));
    }
}
