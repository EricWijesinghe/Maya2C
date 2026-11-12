//! Payment requests and receipts, driven between two clients with no relay
//! (ADR-088). Each threat in the ADR's table that needs a whole exchange is
//! tested here; the field-level ones live beside the code in `pay.rs`.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use maya_chat::ChatError;
use maya_chat::client::{Client, Frame};
use maya_chat::identity::Identity;
use maya_chat::pay::{self, ChainTx, Paid, Request};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;
const ONE_BIT: u32 = 1;

fn payee_id() -> Identity {
    Identity::from_seed([1; 32])
}

fn payer_id() -> Identity {
    Identity::from_seed([2; 32])
}

fn carol_id() -> Identity {
    Identity::from_seed([3; 32])
}

/// A payee and a payer with a session open from the payee's side.
fn connected() -> (Client, Client) {
    let mut payee = Client::new(payee_id());
    let mut payer = Client::new(payer_id());
    let bundle = payer_id().prekey_bundle(1, NOW + DAY).unwrap();
    let id = payee
        .queue(payer_id().address(), Some(&bundle), b"hi", NOW, 1)
        .unwrap();
    let frame = deliver(&mut payee, id, &mut payer, NOW).unwrap();
    assert!(matches!(frame, Frame::Text(_)));
    (payee, payer)
}

/// Sends the queued message `id` from `from` and opens it at `to`.
fn deliver(from: &mut Client, id: [u8; 32], to: &mut Client, now: u64) -> Result<Frame, ChatError> {
    let envelope = from.envelope_for_send(&id, ONE_BIT).unwrap();
    to.open_frame(&envelope, now)
}

fn request(amount: &str, memo: &str) -> Request {
    Request::sign(
        &payee_id(),
        &payer_id().address(),
        amount,
        memo,
        NOW + 600,
        NOW,
    )
    .unwrap()
}

#[test]
fn a_request_reaches_the_payer_with_the_id_the_payee_issued() {
    let (mut payee, mut payer) = connected();
    let req = request("2500", "lunch");
    let issued = payee
        .queue_request(payer_id().address(), &req, NOW, 1)
        .unwrap();
    let frame = deliver(&mut payee, issued, &mut payer, NOW).unwrap();
    let Frame::Pay { from, id, request } = frame else {
        panic!("expected a payment request, got {frame:?}");
    };
    assert_eq!(from, payee_id().address());
    assert_eq!(
        id,
        req.id(&payer_id().address()),
        "both sides compute the same request id"
    );
    assert_eq!(request.amount, "2500");
    assert_eq!(request.memo, "lunch");
    let pending = payer.take_pay_frames();
    assert_eq!(pending.len(), 1, "the frame waits for the caller");
    assert!(payer.take_pay_frames().is_empty());
}

#[test]
fn a_payee_cannot_send_a_request_signed_for_another_account() {
    let (mut payee, _payer) = connected();
    let carols = Request::sign(&carol_id(), &payer_id().address(), "5", "", NOW + 60, NOW).unwrap();
    assert!(
        payee
            .queue_request(payer_id().address(), &carols, NOW, 1)
            .is_err(),
        "a request naming carol cannot leave our chat"
    );
}

#[test]
fn a_request_cannot_open_a_session() {
    let mut payee = Client::new(payee_id());
    let req = request("5", "");
    assert_eq!(
        payee.queue_request(payer_id().address(), &req, NOW, 1),
        Err(ChatError::NoSession)
    );
}

#[test]
fn typed_text_cannot_pose_as_a_request_or_a_receipt() {
    let (mut payee, _payer) = connected();
    let req = request("5", "");
    assert!(
        payee
            .queue(payer_id().address(), None, &req.encode(), NOW, 1)
            .is_err(),
        "a request as plain text"
    );
    let paid = Paid::sign(&payer_id(), &payee_id().address(), [1; 32], [2; 32]).unwrap();
    assert!(
        payee
            .queue(payer_id().address(), None, &paid.encode(), NOW, 1)
            .is_err(),
        "a receipt as plain text"
    );
}

#[test]
fn an_expired_request_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    let req = request("5", "late");
    let issued = payee
        .queue_request(payer_id().address(), &req, NOW, 1)
        .unwrap();
    let late = NOW + 601;
    assert_eq!(
        deliver(&mut payee, issued, &mut payer, late),
        Err(ChatError::Expired)
    );
    assert!(payer.take_pay_frames().is_empty());
}

#[test]
fn a_replayed_envelope_is_refused_and_not_shown_twice() {
    let (mut payee, mut payer) = connected();
    let req = request("5", "once");
    let issued = payee
        .queue_request(payer_id().address(), &req, NOW, 1)
        .unwrap();
    let envelope = payee.envelope_for_send(&issued, ONE_BIT).unwrap();
    assert!(payer.open_frame(&envelope, NOW).is_ok());
    assert!(
        payer.open_frame(&envelope, NOW).is_err(),
        "the same bytes again"
    );
    assert_eq!(payer.take_pay_frames().len(), 1);
}

#[test]
fn a_full_payment_settles_once_on_chain_and_not_twice() {
    let (mut payee, mut payer) = connected();
    let req = request("2500", "lunch");
    let issued = payee
        .queue_request(payer_id().address(), &req, NOW, 1)
        .unwrap();
    deliver(&mut payee, issued, &mut payer, NOW).unwrap();
    let Frame::Pay { request, id, .. } = payer.take_pay_frames().remove(0) else {
        panic!("expected a payment request");
    };

    // The wallet pays `request.payee`, and returns the transaction id.
    let tx = [0x42; 32];
    let receipt = Paid::sign(&payer_id(), &request.payee, id, tx).unwrap();
    let receipt_id = payer
        .queue_paid(payee_id().address(), &receipt, NOW, 1)
        .unwrap();
    let frame = deliver(&mut payer, receipt_id, &mut payee, NOW).unwrap();
    let Frame::Paid {
        from,
        request_id,
        tx_id,
    } = frame
    else {
        panic!("expected a receipt, got {frame:?}");
    };
    assert_eq!(from, payer_id().address());
    assert_eq!((request_id, tx_id), (req.id(&payer_id().address()), tx));

    // The payee checks the chain: the canonical block has the tx paying it.
    let block = vec![ChainTx {
        id: tx,
        outputs: vec![(payee_id().address(), "2500".to_owned())],
    }];
    assert!(pay::settles_on_chain(
        &block,
        &tx_id,
        &payee_id().address(),
        "2500"
    ));
    payee.settle(request_id, tx_id, NOW).unwrap();

    assert_eq!(
        payee.settle(request_id, tx_id, NOW),
        Err(ChatError::Replayed),
        "the same receipt again"
    );
}

#[test]
fn a_receipt_from_someone_other_than_the_payer_is_not_a_payment() {
    let (mut payee, mut payer) = connected();
    let req = request("5", "");
    let issued = payee
        .queue_request(payer_id().address(), &req, NOW, 1)
        .unwrap();
    deliver(&mut payee, issued, &mut payer, NOW).unwrap();
    payer.take_pay_frames();

    // Signed by carol, sent from the payer's session: the signature fails.
    let forged = Paid::sign(
        &carol_id(),
        &payee_id().address(),
        req.id(&payer_id().address()),
        [9; 32],
    )
    .unwrap();
    let id = payer
        .queue_paid(payee_id().address(), &forged, NOW, 1)
        .unwrap();
    assert_eq!(
        deliver(&mut payer, id, &mut payee, NOW),
        Err(ChatError::BadSignature)
    );
    assert!(payee.take_pay_frames().is_empty());
}
