//! Payment requests and receipts, driven between two clients with no relay
//! (ADR-088), and settled against a node in memory over the same JSON-RPC
//! the real node serves. Each threat in the ADR's table that needs a whole
//! exchange is tested here; the field-level ones live beside the code in
//! `pay/mod.rs`, `pay/ledger.rs`, `pay/chain.rs` and `pay_link.rs`.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;

use maya_address::Network;
use maya_chat::client::{Client, Frame};
use maya_chat::identity::Identity;
use maya_chat::pay::binding::{account_of, message};
use maya_chat::pay::chain::{ChainRpc, Scan, find_final};
use maya_chat::pay::{Binding, BindingKind, ChainTag, FinalTx, Paid, Request, Terms};
use maya_chat::pay_link::parse_pay_uri;
use maya_chat::{Address, ChatError};
use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, MasterSeed, SignatureSuite};
use serde_json::{Value, json};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3_600;
const ONE_BIT: u32 = 1;
const CHAIN: ChainTag = [0xc1; 32];
/// The chain tip when the payee issues its request.
const TIP: u64 = 100;
const TX: [u8; 32] = [0x42; 32];

fn payee_id() -> Identity {
    Identity::from_seed([1; 32])
}

fn payer_id() -> Identity {
    Identity::from_seed([2; 32])
}

fn carol_id() -> Identity {
    Identity::from_seed([3; 32])
}

/// The wallet stand-in: what the wallet app signs for `chat_id`, from a key
/// derived from `wallet`. The chat never holds this key.
fn wallet_signs(
    wallet: u8,
    kind: BindingKind,
    chain: ChainTag,
    chat_id: Address,
    (issued_at, expires_at): (u64, u64),
) -> Binding {
    let key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([wallet; 32]));
    let public_key = HybridMlDsa65SlhDsa128s::public_key(&key);
    let account = account_of(&public_key);
    let msg = message(kind, &chain, &chat_id, &account, issued_at, expires_at);
    Binding {
        kind,
        chain,
        chat_id,
        account,
        issued_at,
        expires_at,
        signature: HybridMlDsa65SlhDsa128s::sign(&key, &msg).unwrap(),
        public_key,
    }
}

/// Each test identity's wallet seed byte: 11, 12 and 13 for payee, payer
/// and carol.
fn wallet_of(identity: &Identity) -> u8 {
    let address = identity.address();
    [payee_id(), payer_id(), carol_id()]
        .iter()
        .position(|i| i.address() == address)
        .map_or(99, |n| 11 + u8::try_from(n).unwrap())
}

/// `identity`'s live binding on [`CHAIN`], signed once: hybrid signing is
/// the slow part of these tests.
fn bound(identity: &Identity) -> Binding {
    static CACHE: Mutex<Vec<(Address, Binding)>> = Mutex::new(Vec::new());
    let chat_id = identity.address();
    let mut cache = CACHE.lock().unwrap();
    if let Some((_, b)) = cache.iter().find(|(id, _)| *id == chat_id) {
        return b.clone();
    }
    let b = wallet_signs(
        wallet_of(identity),
        BindingKind::Bind,
        CHAIN,
        chat_id,
        (NOW - 10, NOW + DAY),
    );
    cache.push((chat_id, b.clone()));
    b
}

/// The wallet account `identity`'s requests are paid to and its payments
/// come from.
fn wallet(identity: &Identity) -> Address {
    bound(identity).account
}

fn client(identity: Identity) -> Client {
    let binding = bound(&identity);
    let mut client = Client::new(identity);
    client.set_pay_chain(CHAIN);
    client.set_pay_binding(binding, NOW).unwrap();
    client
}

/// A payee and a payer with a session open from the payee's side.
fn connected() -> (Client, Client) {
    let mut payee = client(payee_id());
    let mut payer = client(payer_id());
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
    let binding = bound(&payee_id());
    let terms = Terms {
        chain: CHAIN,
        binding: &binding,
        amount,
        memo,
        expires_at: NOW + 600,
    };
    Request::sign(&payee_id(), &payer_id().address(), &terms, NOW).unwrap()
}

/// Issues a request for `amount` and delivers it; returns its id.
fn issue(payee: &mut Client, payer: &mut Client, amount: &str) -> [u8; 32] {
    let req = request(amount, "lunch");
    let out = payee
        .queue_request(payer_id().address(), &req, TIP, NOW, 1)
        .unwrap();
    deliver(payee, out, payer, NOW).unwrap();
    req.id(&payer_id().address())
}

/// Sends the payer's receipt for `request_id`, paid by `tx`, and opens it.
fn report(payee: &mut Client, payer: &mut Client, request_id: [u8; 32], tx: [u8; 32]) -> Frame {
    let receipt = paid(&payer_id(), request_id, tx);
    let out = payer
        .queue_paid(payee_id().address(), &receipt, NOW, 1)
        .unwrap();
    deliver(payer, out, payee, NOW).unwrap()
}

/// A receipt signed by `signer`, carrying the payer's binding.
fn paid(signer: &Identity, request_id: [u8; 32], tx: [u8; 32]) -> Paid {
    let binding = bound(&payer_id());
    Paid::sign(
        signer,
        &binding,
        &payee_id().address(),
        &CHAIN,
        request_id,
        tx,
    )
    .unwrap()
}

/// `req` with its binding replaced and the payee's signature redone, as a
/// payee running modified code could send it.
fn rebound(mut req: Request, binding: Binding) -> Request {
    req.pay_to = binding.account;
    req.binding = binding;
    req.signature = payee_id().sign(&req.id(&payer_id().address())).unwrap();
    req
}

/// Queues `req` at the payee and opens it at the payer.
fn send(
    payee: &mut Client,
    payer: &mut Client,
    req: &Request,
    now: u64,
) -> Result<Frame, ChatError> {
    let out = payee
        .queue_request(payer_id().address(), req, TIP, NOW, 1)
        .unwrap();
    deliver(payee, out, payer, now)
}

/// A node in memory serving `get_chain_info`, `get_checkpoint` and
/// `get_block_by_height` as the real node shapes them.
struct FakeNode {
    finalized: u64,
    blocks: BTreeMap<u64, Value>,
}

impl ChainRpc for FakeNode {
    async fn call(&self, method: &str, params: Value) -> Result<Value, ChatError> {
        let missing = || ChatError::Network("no such block".to_owned());
        match method {
            "get_chain_info" => Ok(json!({"genesis": hex::encode(CHAIN), "height": 0})),
            "get_checkpoint" => {
                Ok(json!({"epoch": 1, "height": self.finalized, "block": "", "signatures": []}))
            }
            "get_block_by_height" => self
                .blocks
                .get(&params[0].as_u64().unwrap())
                .cloned()
                .ok_or_else(missing),
            _ => Err(ChatError::Network("unknown method".to_owned())),
        }
    }
}

/// A chain where block `at` holds `TX`, sent by `sender`, paying `to`.
fn chain_with(at: u64, sender: &Address, to: &Address, amount: &str) -> FakeNode {
    let mut blocks = BTreeMap::new();
    for h in 1..=200 {
        let txs = if h == at {
            vec![json!({
                "txid": hex::encode(TX),
                "sender": hex::encode(sender),
                "outputs": [{"recipient": hex::encode(to), "amount": amount}],
            })]
        } else {
            vec![]
        };
        blocks.insert(
            h,
            json!({"height": h, "header": {}, "transactions": txs, "raw": ""}),
        );
    }
    FakeNode {
        finalized: 200,
        blocks,
    }
}

fn run<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

/// What the payee's node says about `TX`, searched from above the request's
/// issue height.
fn look_up(payee: &Client, request_id: &[u8; 32], node: &FakeNode) -> Option<FinalTx> {
    let from = payee.issued_height(request_id).unwrap() + 1;
    match run(find_final(node, &CHAIN, &TX, from)).unwrap() {
        Scan::Found(found) => Some(found),
        Scan::NotYet { .. } => None,
    }
}

#[test]
fn a_request_reaches_the_payer_with_the_id_the_payee_issued() {
    let (mut payee, mut payer) = connected();
    let req = request("2500", "lunch");
    let issued = payee
        .queue_request(payer_id().address(), &req, TIP, NOW, 1)
        .unwrap();
    let frame = deliver(&mut payee, issued, &mut payer, NOW).unwrap();
    let Frame::Pay { from, id, request } = frame else {
        panic!("expected a payment request, got {frame:?}");
    };
    assert_eq!(from, payee_id().address());
    assert_eq!(
        id,
        req.id(&payer_id().address()),
        "both sides agree on the id"
    );
    assert_eq!(
        (request.amount.as_str(), request.memo.as_str()),
        ("2500", "lunch")
    );
    assert_eq!(
        payer.take_pay_frames().len(),
        1,
        "the frame waits for the caller"
    );
    assert!(payer.take_pay_frames().is_empty());
}

#[test]
fn a_full_final_payment_from_the_payer_settles_once() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "2500");
    let link = payer.pay_link(&id, Network::Testnet, NOW).unwrap();
    let parsed = parse_pay_uri(&link, Network::Testnet).unwrap();
    assert_eq!(
        (parsed.to, parsed.chain, parsed.request_id),
        (wallet(&payee_id()), CHAIN, id)
    );

    let Frame::Paid {
        from,
        request_id,
        tx_id,
    } = report(&mut payee, &mut payer, id, TX)
    else {
        panic!("expected a receipt");
    };
    // Wallet to wallet: neither chat address moves money.
    let node = chain_with(TIP + 5, &wallet(&payer_id()), &wallet(&payee_id()), "2500");
    let found = look_up(&payee, &request_id, &node).unwrap();
    payee.settle(&from, request_id, tx_id, &found, NOW).unwrap();
    assert_eq!(
        payee.settle(&from, request_id, tx_id, &found, NOW),
        Err(ChatError::Replayed),
        "the same receipt again"
    );
}

#[test]
fn a_public_payment_to_the_payee_from_someone_else_does_not_settle() {
    // H1: an exchange pays the payee; the payer names that transaction.
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    let Frame::Paid {
        from,
        request_id,
        tx_id,
    } = report(&mut payee, &mut payer, id, TX)
    else {
        panic!("expected a receipt");
    };
    let exchange = carol_id().address();
    let node = chain_with(TIP + 5, &exchange, &wallet(&payee_id()), "1000000");
    let found = look_up(&payee, &request_id, &node).unwrap();
    assert_eq!(
        payee.settle(&from, request_id, tx_id, &found, NOW),
        Err(ChatError::Refused(
            "transaction was not sent by the payer's wallet"
        ))
    );
}

#[test]
fn a_payment_from_before_the_request_is_not_searched_or_settled() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    report(&mut payee, &mut payer, id, TX);
    let node = chain_with(TIP, &wallet(&payer_id()), &wallet(&payee_id()), "5");
    assert_eq!(look_up(&payee, &id, &node), None);
}

#[test]
fn a_payment_not_yet_final_is_not_found() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    let mut node = chain_with(TIP + 50, &wallet(&payer_id()), &wallet(&payee_id()), "5");
    node.finalized = TIP + 49;
    assert_eq!(look_up(&payee, &id, &node), None);
}

#[test]
fn a_receipt_from_someone_other_than_the_payer_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    payer.take_pay_frames();
    let forged = paid(&carol_id(), id, TX);
    let out = payer
        .queue_paid(payee_id().address(), &forged, NOW, 1)
        .unwrap();
    assert_eq!(
        deliver(&mut payer, out, &mut payee, NOW),
        Err(ChatError::BadSignature)
    );
    assert!(payee.take_pay_frames().is_empty());
}

#[test]
fn a_receipt_for_a_request_never_issued_to_that_payer_is_refused() {
    let (_payee, mut payer) = connected();
    let receipt = paid(&payer_id(), [9; 32], TX);
    assert!(
        payer
            .queue_paid(payee_id().address(), &receipt, NOW, 1)
            .is_err(),
        "the payer's own chat refuses a receipt for a request it never received"
    );
}

#[test]
fn the_payer_cannot_pay_twice_or_pay_an_expired_request() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    assert_eq!(
        payer.pay_link(&id, Network::Testnet, NOW + 600),
        Err(ChatError::Expired)
    );
    assert!(payer.pay_link(&id, Network::Testnet, NOW).is_ok());
    report(&mut payee, &mut payer, id, TX);
    assert_eq!(
        payer.pay_link(&id, Network::Testnet, NOW),
        Err(ChatError::Replayed),
        "already paid"
    );
    let again = paid(&payer_id(), id, [7; 32]);
    assert_eq!(
        payer.queue_paid(payee_id().address(), &again, NOW, 1),
        Err(ChatError::Replayed)
    );
}

#[test]
fn a_request_for_another_chain_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    payer.set_pay_chain([0xc2; 32]);
    let out = payee
        .queue_request(payer_id().address(), &request("5", ""), TIP, NOW, 1)
        .unwrap();
    assert_eq!(
        deliver(&mut payee, out, &mut payer, NOW),
        Err(ChatError::Refused("request is for another chain"))
    );
}

#[test]
fn payment_state_survives_a_restart_before_it_is_taken() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    // Both sides crash after opening and saving, before the caller acts.
    let payer_saved = payer.save().unwrap();
    let mut payer = Client::restore(payer_id(), &payer_saved).unwrap();
    let frames = payer.take_pay_frames();
    assert!(
        matches!(frames.as_slice(), [Frame::Pay { id: got, .. }] if *got == id),
        "the request waits across the restart: {frames:?}"
    );
    report(&mut payee, &mut payer, id, TX);
    let payee_saved = payee.save().unwrap();
    let mut payee = Client::restore(payee_id(), &payee_saved).unwrap();
    assert_eq!(payee.issued_height(&id), Some(TIP), "the issue survives");
    assert_eq!(payee.take_pay_frames().len(), 1, "the receipt survives");
}

#[test]
fn a_payee_cannot_send_a_request_signed_for_another_account() {
    let (mut payee, _payer) = connected();
    let carol_binding = bound(&carol_id());
    let terms = Terms {
        chain: CHAIN,
        binding: &carol_binding,
        amount: "5",
        memo: "",
        expires_at: NOW + 60,
    };
    let carols = Request::sign(&carol_id(), &payer_id().address(), &terms, NOW).unwrap();
    assert!(
        payee
            .queue_request(payer_id().address(), &carols, TIP, NOW, 1)
            .is_err(),
        "a request naming carol cannot leave our chat"
    );
}

#[test]
fn a_request_cannot_open_a_session() {
    let mut payee = client(payee_id());
    assert_eq!(
        payee.queue_request(payer_id().address(), &request("5", ""), TIP, NOW, 1),
        Err(ChatError::NoSession)
    );
}

#[test]
fn typed_text_cannot_pose_as_a_request_or_a_receipt() {
    let (mut payee, _payer) = connected();
    let as_text = request("5", "").encode();
    assert!(
        payee
            .queue(payer_id().address(), None, &as_text, NOW, 1)
            .is_err()
    );
    let paid = paid(&payer_id(), [1; 32], TX);
    assert!(
        payee
            .queue(payer_id().address(), None, &paid.encode(), NOW, 1)
            .is_err()
    );
}

#[test]
fn an_expired_request_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    let out = payee
        .queue_request(payer_id().address(), &request("5", "late"), TIP, NOW, 1)
        .unwrap();
    assert_eq!(
        deliver(&mut payee, out, &mut payer, NOW + 601),
        Err(ChatError::Expired)
    );
    assert!(payer.take_pay_frames().is_empty());
}

#[test]
fn a_replayed_envelope_is_refused_and_not_shown_twice() {
    let (mut payee, mut payer) = connected();
    let out = payee
        .queue_request(payer_id().address(), &request("5", "once"), TIP, NOW, 1)
        .unwrap();
    let envelope = payee.envelope_for_send(&out, ONE_BIT).unwrap();
    assert!(payer.open_frame(&envelope, NOW).is_ok());
    assert!(
        payer.open_frame(&envelope, NOW).is_err(),
        "the same bytes again"
    );
    assert_eq!(payer.take_pay_frames().len(), 1);
}

// --- wallet bindings (ADR-088 amendment 1, H2) ---

#[test]
fn a_request_with_a_forged_binding_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    let mut forged = bound(&payee_id());
    forged.signature[100] ^= 1;
    let req = rebound(request("5", ""), forged);
    assert_eq!(
        send(&mut payee, &mut payer, &req, NOW),
        Err(ChatError::BadSignature)
    );
    // A thief's own wallet relabelled with the payee's account.
    let live = (NOW, NOW + DAY);
    let chat = payee_id().address();
    let mut relabelled = wallet_signs(66, BindingKind::Bind, CHAIN, chat, live);
    relabelled.account = wallet(&payee_id());
    let req = rebound(request("5", "x"), relabelled);
    assert_eq!(
        send(&mut payee, &mut payer, &req, NOW),
        Err(ChatError::BadSignature)
    );
    assert!(payer.take_pay_frames().is_empty());
}

#[test]
fn a_request_carrying_another_chat_identitys_binding_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    let req = rebound(request("5", ""), bound(&carol_id()));
    assert_eq!(
        send(&mut payee, &mut payer, &req, NOW),
        Err(ChatError::Refused("binding is for another chat identity"))
    );
}

#[test]
fn a_request_carrying_another_chains_binding_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    let chat = payee_id().address();
    let testnet = wallet_signs(11, BindingKind::Bind, [0xc2; 32], chat, (NOW, NOW + DAY));
    let req = rebound(request("5", ""), testnet);
    assert_eq!(
        send(&mut payee, &mut payer, &req, NOW),
        Err(ChatError::Refused("binding is for another chain"))
    );
}

#[test]
fn a_request_whose_binding_expired_is_refused_on_arrival() {
    let (mut payee, mut payer) = connected();
    let chat = payee_id().address();
    let short = wallet_signs(11, BindingKind::Bind, CHAIN, chat, (NOW - 10, NOW + 300));
    let req = rebound(request("5", ""), short);
    assert_eq!(
        send(&mut payee, &mut payer, &req, NOW + 300),
        Err(ChatError::Expired),
        "the request itself is live until NOW + 600"
    );
}

#[test]
fn a_revocation_reaches_contacts_and_old_requests_stop_being_payable() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    payer.take_pay_frames();
    assert!(payer.pay_link(&id, Network::Testnet, NOW).is_ok());

    let chat = payee_id().address();
    let revoke = wallet_signs(11, BindingKind::Revoke, CHAIN, chat, (NOW, NOW + DAY));
    payee.set_pay_binding(revoke.clone(), NOW).unwrap();
    assert_eq!(
        payee.pay_binding(),
        None,
        "the payee stops naming the account"
    );
    let out = payee
        .queue_binding(payer_id().address(), &revoke, NOW, 1)
        .unwrap();
    let frame = deliver(&mut payee, out, &mut payer, NOW).unwrap();
    assert!(
        matches!(frame, Frame::Binding { from, revoked: true } if from == chat),
        "{frame:?}"
    );
    assert_eq!(
        payer.pay_link(&id, Network::Testnet, NOW),
        Err(ChatError::Refused(
            "the payee's binding was replaced or revoked"
        )),
        "a request held from before the revocation is not paid"
    );
    // A copy of the payee's chat that kept the old binding is refused too.
    assert_eq!(
        send(&mut payee, &mut payer, &request("5", "stale"), NOW),
        Err(ChatError::Refused("binding is superseded by a newer one"))
    );
}

#[test]
fn a_newer_binding_to_a_new_wallet_supersedes_the_old_one() {
    let (mut payee, mut payer) = connected();
    let chat = payee_id().address();
    let moved = wallet_signs(21, BindingKind::Bind, CHAIN, chat, (NOW, NOW + DAY));
    payee.set_pay_binding(moved.clone(), NOW).unwrap();
    let terms = Terms {
        chain: CHAIN,
        binding: &moved,
        amount: "5",
        memo: "",
        expires_at: NOW + 60,
    };
    let req = Request::sign(&payee_id(), &payer_id().address(), &terms, NOW).unwrap();
    let Frame::Pay { request, .. } = send(&mut payee, &mut payer, &req, NOW).unwrap() else {
        panic!("expected a request");
    };
    assert_eq!(request.pay_to, moved.account);
    assert_eq!(
        send(&mut payee, &mut payer, &self::request("5", "old"), NOW),
        Err(ChatError::Refused("binding is superseded by a newer one"))
    );
}

#[test]
fn a_payment_from_the_payers_chat_address_does_not_settle() {
    let (mut payee, mut payer) = connected();
    let id = issue(&mut payee, &mut payer, "5");
    let Frame::Paid {
        from,
        request_id,
        tx_id,
    } = report(&mut payee, &mut payer, id, TX)
    else {
        panic!("expected a receipt");
    };
    let node = chain_with(TIP + 5, &payer_id().address(), &wallet(&payee_id()), "5");
    let found = look_up(&payee, &request_id, &node).unwrap();
    assert_eq!(
        payee.settle(&from, request_id, tx_id, &found, NOW),
        Err(ChatError::Refused(
            "transaction was not sent by the payer's wallet"
        ))
    );
}

#[test]
fn a_binding_for_another_identity_cannot_be_installed() {
    let mut payee = client(payee_id());
    assert_eq!(
        payee.set_pay_binding(bound(&carol_id()), NOW),
        Err(ChatError::Refused("binding is for another chat identity"))
    );
    let mut fresh = Client::new(payee_id());
    assert_eq!(
        fresh.set_pay_binding(bound(&payee_id()), NOW),
        Err(ChatError::Refused("no chain set for payments"))
    );
}
