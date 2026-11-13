//! Chat mini-apps (ADR-138): the signed manifest, the permission allow-list,
//! and the host bridge. Each test fails if the rule it names is removed.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use std::sync::OnceLock;

use maya_address::{Network, encode};
use maya_chat::ChatError;
use maya_chat::miniapp::{Manifest, Permission, Verified};
use maya_chat::miniapp_bridge::{Bridge, Call, Reply};
use maya_chat::pay_link::parse_pay_uri;
use maya_crypto_pq::suite::MasterSeed;

const HTML: &[u8] = b"<!doctype html><p>split the bill</p>";
const PAYEE: [u8; 32] = [7; 32];
const USER: [u8; 32] = [9; 32];
const CHAIN: [u8; 32] = [0xc1; 32];
const ONE_COIN_BASE: &str = "1000000000000000000000000";

fn publisher() -> MasterSeed {
    MasterSeed::from_bytes([5; 32])
}

/// One signed manifest for the whole file: hybrid signing is the slow step.
fn good() -> &'static Manifest {
    static M: OnceLock<Manifest> = OnceLock::new();
    M.get_or_init(|| {
        Manifest::sign(
            &publisher(),
            "Split",
            blake3::hash(b"icon").into(),
            blake3::hash(HTML).into(),
            &["read-address", "request-payment", "send-message"],
        )
        .unwrap()
    })
}

fn verified() -> Verified {
    good().verify().unwrap()
}

fn bridge() -> Bridge {
    Bridge::new(verified(), Network::Testnet, CHAIN, USER).unwrap()
}

fn pay_call() -> Call {
    Call::Pay {
        to: encode(Network::Testnet, &PAYEE),
        amount: ONE_COIN_BASE.into(),
        memo: "pizza".into(),
    }
}

/// Answers every prompt the bridge raises with `confirm`, as the UI would,
/// and returns the final reply plus the prompt texts the user saw.
fn drive(
    b: &mut Bridge,
    call: Call,
    mut confirm: impl FnMut(&str) -> bool,
) -> (Reply, Vec<String>) {
    let token = b.token();
    let mut seen = Vec::new();
    let mut reply = b.handle(&token, call).unwrap();
    while let Reply::Ask { prompt, text } = reply {
        let yes = confirm(&text);
        seen.push(text);
        reply = b.answer(&token, prompt, yes).unwrap();
    }
    (reply, seen)
}

#[test]
fn a_good_manifest_verifies_and_round_trips() {
    let v = verified();
    assert_eq!(v.name, "Split");
    assert!(v.permissions.contains(&Permission::RequestPayment));
    assert!(v.entry_matches(HTML));
    assert!(!v.entry_matches(b"<p>something else</p>"));
    let back = Manifest::from_bytes(&good().to_bytes().unwrap()).unwrap();
    assert_eq!(back.verify().unwrap(), v);
}

#[test]
fn a_manifest_with_a_bad_signature_is_refused() {
    let mut m = good().clone();
    m.signature[0] ^= 1;
    assert_eq!(m.verify(), Err(ChatError::BadSignature));

    let mut renamed = good().clone();
    renamed.name = "Spilt".into();
    assert_eq!(renamed.verify(), Err(ChatError::BadSignature));

    let mut swapped = good().clone();
    swapped.entry_hash = blake3::hash(b"<script>steal()</script>").into();
    assert_eq!(swapped.verify(), Err(ChatError::BadSignature));

    let mut short_key = good().clone();
    short_key.publisher_key.pop();
    assert_eq!(short_key.verify(), Err(ChatError::BadSignature));
}

#[test]
fn a_permission_outside_the_allow_list_is_refused() {
    let refused = Manifest::sign(&publisher(), "Net", [0; 32], [0; 32], &["network"]);
    assert!(matches!(refused, Err(ChatError::Refused(_))));
    assert!(Permission::parse("sign-transaction").is_err());

    // Added after signing: the allow-list fires before the signature does.
    let mut m = good().clone();
    m.permissions.push("camera".into());
    assert!(matches!(m.verify(), Err(ChatError::Refused(_))));

    let mut dup = good().clone();
    dup.permissions.push("send-message".into());
    assert!(matches!(dup.verify(), Err(ChatError::Refused(_))));
}

#[test]
fn the_bridge_drops_messages_from_an_unknown_origin() {
    let mut b = bridge();
    let stranger = [0xAA; 16];
    assert_ne!(stranger, b.token());
    assert_eq!(b.handle(&stranger, Call::Address), None);
    assert_eq!(b.handle(&stranger, pay_call()), None);

    // A prompt raised for the real frame cannot be answered by another one.
    let token = b.token();
    let Some(Reply::Ask { prompt, .. }) = b.handle(&token, Call::Address) else {
        panic!("expected a permission prompt");
    };
    assert_eq!(b.answer(&stranger, prompt, true), None);
}

#[test]
fn the_pay_flow_needs_user_confirmation() {
    // Every prompt declined: no wallet link ever comes back.
    let mut b = bridge();
    let (reply, _) = drive(&mut b, pay_call(), |_| false);
    assert_eq!(reply, Reply::Declined);

    // Permission granted, payment declined: still no link.
    let mut b = bridge();
    let (reply, seen) = drive(&mut b, pay_call(), |text| text.starts_with("Allow"));
    assert_eq!(reply, Reply::Declined);
    assert_eq!(seen.len(), 2);

    // Both confirmed: a wallet link, and the confirm text named the payee in
    // full, the amount in coins and the memo before the user said yes.
    let mut b = bridge();
    let (reply, seen) = drive(&mut b, pay_call(), |_| true);
    let Reply::OpenWallet { link: uri } = reply else {
        panic!("expected a wallet link, got {reply:?}");
    };
    let link = parse_pay_uri(&uri, Network::Testnet).unwrap();
    assert_eq!(link.to, PAYEE);
    assert_eq!(link.amount, ONE_COIN_BASE);
    assert_eq!(
        link.chain, CHAIN,
        "the wallet refuses a link for another chain"
    );
    let confirm = seen.last().unwrap();
    assert!(confirm.contains(&encode(Network::Testnet, &PAYEE)));
    assert!(confirm.contains("Amount: 1 "));
    assert!(confirm.contains("pizza"));

    // Granted once: the next payment asks only the pay confirmation.
    let (_, seen) = drive(&mut b, pay_call(), |_| true);
    assert_eq!(seen.len(), 1);
}

#[test]
fn an_undeclared_permission_is_refused_without_asking() {
    let m = Manifest::sign(&publisher(), "Read", [1; 32], [2; 32], &["read-address"]).unwrap();
    let mut b = Bridge::new(m.verify().unwrap(), Network::Testnet, CHAIN, USER).unwrap();
    let (reply, seen) = drive(&mut b, pay_call(), |_| true);
    assert!(matches!(reply, Reply::Refused { .. }));
    assert!(seen.is_empty());
}

#[test]
fn a_mini_app_message_cannot_pose_as_a_payment() {
    let mut b = bridge();
    let fake = String::from_utf8_lossy(maya_chat::pay::MAGIC).into_owned();
    let (reply, _) = drive(&mut b, Call::Send { text: fake }, |_| true);
    assert!(matches!(reply, Reply::Refused { .. }));

    let (reply, _) = drive(
        &mut b,
        Call::Send {
            text: "hello".into(),
        },
        |_| true,
    );
    assert_eq!(
        reply,
        Reply::Send {
            text: "[Split] hello".into()
        }
    );
}

#[test]
fn a_bad_pay_call_is_refused_before_any_prompt() {
    let mut b = bridge();
    let bad = [
        Call::Pay {
            to: "not-an-address".into(),
            amount: ONE_COIN_BASE.into(),
            memo: String::new(),
        },
        Call::Pay {
            to: encode(Network::Mainnet, &PAYEE),
            amount: ONE_COIN_BASE.into(),
            memo: String::new(),
        },
        Call::Pay {
            to: encode(Network::Testnet, &PAYEE),
            amount: "0".into(),
            memo: String::new(),
        },
        Call::Pay {
            to: encode(Network::Testnet, &PAYEE),
            amount: "1".into(),
            memo: "\u{202E}gift".into(),
        },
    ];
    for call in bad {
        let (reply, seen) = drive(&mut b, call, |_| true);
        assert!(matches!(reply, Reply::Refused { .. }), "{reply:?}");
        assert!(seen.is_empty());
    }
}
