#![allow(clippy::unwrap_used)]

use maya_interop::eth::{self, Header};
use maya_interop::intents::{DeliveryVerifier, Escrow, Intent, IntentError, State};
use maya_interop::routes::{Route, RouteError};

fn h32(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

fn hexs(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        use std::fmt::Write;
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Ethereum mainnet's genesis header. Nonce, extra data, gas limit and
/// difficulty are as pinned in go-ethereum's `core/genesis.go`
/// (`DefaultGenesisBlock`); the empty-list roots are the well-known
/// constants; the state root is the mainnet genesis allocation's.
fn mainnet_genesis() -> Header {
    Header {
        ommers_hash: h32("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
        state_root: h32("d7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544"),
        transactions_root: h32("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"),
        receipts_root: h32("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"),
        logs_bloom: vec![0; 256],
        difficulty: 17_179_869_184,
        gas_limit: 5_000,
        extra_data: h32("11bbe8db4e347b4e8c937c1c8370e4b5ed33adb3db69cbdb7a38e1e50b1b82fa")
            .to_vec(),
        nonce: 66u64.to_be_bytes(),
        ..Header::default()
    }
}

/// go-ethereum `params/config.go`: `MainnetGenesisHash`.
const MAINNET_GENESIS_HASH: &str =
    "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3";

#[test]
fn the_real_mainnet_genesis_header_hashes_to_its_published_hash() {
    let got = eth::hash(&mainnet_genesis());
    println!("keccak256(rlp(mainnet genesis header)) = {}", hexs(&got));
    assert_eq!(hexs(&got), MAINNET_GENESIS_HASH);
}

#[test]
fn a_forged_field_or_a_broken_parent_link_is_caught() {
    let mut forged = mainnet_genesis();
    forged.state_root[0] ^= 1;
    assert_ne!(
        hexs(&eth::hash(&forged)),
        MAINNET_GENESIS_HASH,
        "one flipped bit changes the hash"
    );
    let genesis = mainnet_genesis();
    let child = Header {
        parent_hash: eth::hash(&genesis),
        number: 1,
        ..mainnet_genesis()
    };
    assert!(eth::check_chain(&[genesis.clone(), child.clone()]).is_ok());
    let orphan = Header {
        parent_hash: [7; 32],
        ..child
    };
    assert_eq!(
        eth::check_chain(&[genesis, orphan]),
        Err(1),
        "a header that does not link to its parent is rejected"
    );
}

#[test]
fn a_route_bug_cannot_move_more_than_its_cap_and_trust_grows_only_with_clean_time() {
    let day = 7_200;
    let mut r = Route::new(10_000, 1_000, 100_000, day, day, 0);
    // A bug lets an attacker send repeatedly on day zero: capped at 10,000 in the window.
    let stolen: u128 = (0..100)
        .filter_map(|_| r.send(1_000, 10).ok().map(|()| 1_000))
        .sum();
    assert_eq!(stolen, 10_000);
    assert!(matches!(
        r.send(1, 10),
        Err(RouteError::OverCap { cap: 10_000, .. })
    ));
    // Thirty clean days later the cap has grown; an incident drops it back.
    assert_eq!(r.cap(30 * day), 40_000);
    r.incident(30 * day);
    assert_eq!(r.cap(30 * day + 1), 10_000);
    assert_eq!(r.cap(1_000 * day), 100_000, "never above the ceiling");
}

/// A stand-in light client: accepts a proof equal to the intent's commitment.
/// Real delivery proofs come from the destination chain's light client.
struct Verifier;

impl DeliveryVerifier for Verifier {
    fn verify(&self, intent: &Intent, proof: &[u8]) -> bool {
        proof == [intent.recipient.as_slice(), &intent.amount.to_le_bytes()].concat()
    }
}

fn intent() -> Intent {
    Intent {
        user: [1; 32],
        recipient: b"alice@eth".to_vec(),
        amount: 100,
        escrow: 101,
        deadline: 1_000,
    }
}

#[test]
fn a_solver_that_takes_the_intent_and_never_delivers_gets_nothing_and_the_user_is_refunded() {
    let mut e = Escrow::open(intent());
    assert_eq!(
        e.claim(b"trust me", &Verifier, 500),
        Err(IntentError::BadProof)
    );
    assert_eq!(e.refund(999), Err(IntentError::NotExpired));
    assert_eq!(e.refund(1_001), Ok(101));
    assert_eq!(e.state, State::Refunded);
    let proof = [b"alice@eth".as_slice(), &100u128.to_le_bytes()].concat();
    assert_eq!(
        e.claim(&proof, &Verifier, 1_002),
        Err(IntentError::Closed),
        "no late claim after a refund"
    );
}

#[test]
fn delivery_settles_whoever_submits_the_proof_so_relayer_censorship_does_not_block_it() {
    let mut e = Escrow::open(intent());
    let proof = [b"alice@eth".as_slice(), &100u128.to_le_bytes()].concat();
    // The solver's relayer censors; the user (or anyone) submits the same proof.
    assert_eq!(e.claim(&proof, &Verifier, 900), Ok(101));
    assert_eq!(
        e.refund(2_000),
        Err(IntentError::Closed),
        "no double spend by refunding after settlement"
    );
}
