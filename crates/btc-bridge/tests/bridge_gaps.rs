//! Gap tests (2026-11-10 scan): overflow leaves the books unchanged, open
//! releases stay exact when a payout is refused, the transaction parser
//! refuses truncated and non-canonical input, and wrapped supply plus owed
//! BTC always equals what the bridge saw locked.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use maya_btc_bridge::tx::{self, OP_RETURN};
use maya_btc_bridge::{Bridge, BridgeError, InclusionProof};
use maya_btc_spv::{Header, HeaderChain, Params, merkle_root, prove, sha256d};
use proptest::prelude::*;

const LOCK: &[u8] = &[0x00, 0x14, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA];
const PAYOUT: &[u8] = &[0x00, 0x14, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB];
const ALICE: [u8; 32] = [0xA1; 32];
const DEPTH: u32 = 2;

fn transaction(seed: u32, outputs: &[(u64, &[u8])]) -> Vec<u8> {
    let mut raw = vec![2, 0, 0, 0, 1];
    raw.extend_from_slice(&[0u8; 28]);
    raw.extend_from_slice(&seed.to_le_bytes());
    raw.extend_from_slice(&[0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
    raw.push(u8::try_from(outputs.len()).unwrap());
    for (value, script) in outputs {
        raw.extend_from_slice(&value.to_le_bytes());
        raw.push(u8::try_from(script.len()).unwrap());
        raw.extend_from_slice(script);
    }
    raw.extend_from_slice(&[0, 0, 0, 0]);
    raw
}

fn recipient(who: [u8; 32]) -> Vec<u8> {
    [&[OP_RETURN, 32][..], &who].concat()
}

fn mine(prev: [u8; 32], time: u32, root: [u8; 32]) -> Header {
    let mut h = Header {
        version: 0x2000_0000,
        prev,
        merkle_root: root,
        time,
        bits: 0x207f_ffff,
        nonce: 0,
    };
    while !h.meets_own_target() {
        h.nonce += 1;
    }
    h
}

struct Net {
    bridge: Bridge,
    tip: [u8; 32],
    time: u32,
    seed: u32,
}

impl Net {
    fn new() -> Self {
        let genesis = mine([0; 32], 1_600_000_000, sha256d(b"genesis"));
        let headers = HeaderChain::from_checkpoint(Params::REGTEST, 0, genesis);
        Self {
            bridge: Bridge::new(headers, LOCK.to_vec(), DEPTH),
            tip: genesis.hash(),
            time: 1_600_000_000,
            seed: 1,
        }
    }

    /// Mines `outputs` into a block, buries it, returns its proof.
    fn confirmed(&mut self, outputs: &[(u64, &[u8])]) -> InclusionProof {
        self.seed += 1;
        let coinbase = transaction(0, &[(1, b"\x51".as_slice())]);
        let raw = transaction(self.seed, outputs);
        let all = [coinbase, raw.clone()];
        let ids: Vec<[u8; 32]> = all.iter().map(|t| tx::parse(t).unwrap().txid).collect();
        let mut hash = self.extend(merkle_root(&ids));
        let block = hash;
        for _ in 0..DEPTH {
            hash = self.extend(sha256d(&hash));
        }
        self.tip = hash;
        InclusionProof {
            block,
            raw_tx: raw,
            merkle: prove(&ids, 1).unwrap(),
        }
    }

    fn extend(&mut self, root: [u8; 32]) -> [u8; 32] {
        self.time += 600;
        let h = mine(self.tip, self.time, root);
        self.bridge.headers.add(h).unwrap();
        self.tip = h.hash();
        self.tip
    }

    fn books_balance(&self) {
        assert_eq!(
            self.bridge.supply() + self.bridge.owed(),
            self.bridge.locked()
        );
    }
}

#[test]
fn a_deposit_past_the_u64_cap_is_refused_and_leaves_the_books_unchanged() {
    let mut net = Net::new();
    let big = net.confirmed(&[(u64::MAX - 10, LOCK), (0, &recipient(ALICE))]);
    assert_eq!(net.bridge.mint(&big), Ok((ALICE, u64::MAX - 10)));

    // Pushes `locked` (and ALICE's balance) past u64::MAX.
    let over = net.confirmed(&[(11, LOCK), (0, &recipient(ALICE))]);
    assert_eq!(net.bridge.mint(&over), Err(BridgeError::Overflow));
    assert_eq!(net.bridge.balance(&ALICE), u64::MAX - 10);
    assert_eq!(net.bridge.locked(), u64::MAX - 10);
    assert_eq!(net.bridge.supply(), u64::MAX - 10);
    net.books_balance();

    // The refusal did not burn the transaction: exactly the cap still fits.
    let exact = net.confirmed(&[(10, LOCK), (0, &recipient(ALICE))]);
    assert_eq!(net.bridge.mint(&exact), Ok((ALICE, 10)));
    assert_eq!(net.bridge.balance(&ALICE), u64::MAX);
    assert_eq!(net.bridge.mint(&over), Err(BridgeError::Overflow));
    net.books_balance();
}

#[test]
fn two_lock_outputs_that_sum_past_the_cap_are_refused_whole() {
    let mut net = Net::new();
    let both = net.confirmed(&[
        (u64::MAX / 2 + 1, LOCK),
        (u64::MAX / 2 + 1, LOCK),
        (0, &recipient(ALICE)),
    ]);
    assert_eq!(net.bridge.mint(&both), Err(BridgeError::Overflow));
    assert_eq!((net.bridge.supply(), net.bridge.locked()), (0, 0));
}

#[test]
fn open_releases_are_exact_after_refused_short_and_closing_payouts() {
    let mut net = Net::new();
    let dep = net.confirmed(&[(100_000, LOCK), (0, &recipient(ALICE))]);
    net.bridge.mint(&dep).unwrap();
    let id = net.bridge.burn(ALICE, 60_000, PAYOUT.to_vec()).unwrap();
    let kept = net.bridge.open_releases().clone();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[&id].amount, 60_000);
    assert_eq!(kept[&id].script, PAYOUT);
    assert_eq!(kept[&id].burner, ALICE);

    // A partial payout does not shrink or close the release.
    let part = net.confirmed(&[(25_000, PAYOUT)]);
    assert_eq!(
        net.bridge.confirm_release(id, &part),
        Err(BridgeError::DoesNotPay)
    );
    assert_eq!(net.bridge.open_releases(), &kept);
    assert_eq!(net.bridge.owed(), 60_000);
    assert_eq!(net.bridge.locked(), 100_000);

    // Same-amount output to another script is not a payout.
    let wrong = net.confirmed(&[(60_000, LOCK[..5].as_ref())]);
    assert_eq!(
        net.bridge.confirm_release(id, &wrong),
        Err(BridgeError::DoesNotPay)
    );
    assert_eq!(net.bridge.open_releases(), &kept);

    // Two outputs to the script add up (>= amount closes it).
    let split = net.confirmed(&[(25_000, PAYOUT), (35_000, PAYOUT)]);
    net.bridge.confirm_release(id, &split).unwrap();
    assert!(net.bridge.open_releases().is_empty());
    assert_eq!(net.bridge.owed(), 0);
    assert_eq!(net.bridge.locked(), 40_000);
    net.books_balance();

    // Unknown ids change nothing.
    assert_eq!(
        net.bridge.confirm_release(id + 1, &split),
        Err(BridgeError::UnknownRelease(id + 1))
    );
}

#[test]
fn burn_of_the_whole_balance_succeeds_and_one_more_satoshi_fails_untouched() {
    let mut net = Net::new();
    let dep = net.confirmed(&[(7, LOCK), (0, &recipient(ALICE))]);
    net.bridge.mint(&dep).unwrap();
    assert_eq!(
        net.bridge.burn(ALICE, 8, PAYOUT.to_vec()),
        Err(BridgeError::Insufficient)
    );
    assert_eq!(net.bridge.balance(&ALICE), 7);
    assert!(net.bridge.open_releases().is_empty());
    net.bridge.burn(ALICE, 7, PAYOUT.to_vec()).unwrap();
    assert_eq!(net.bridge.balance(&ALICE), 0);
    net.books_balance();
}

/// A one-output transaction used by the parser tests.
fn simple() -> Vec<u8> {
    transaction(5, &[(1, LOCK)])
}

#[test]
fn every_truncation_of_a_segwit_transaction_is_an_error_not_a_panic() {
    let mut raw = vec![2, 0, 0, 0, 0, 1, 1];
    raw.extend_from_slice(&[7u8; 32]);
    raw.extend_from_slice(&[0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
    raw.push(1);
    raw.extend_from_slice(&5u64.to_le_bytes());
    raw.push(2);
    raw.extend_from_slice(&[0x51, 0x51]);
    raw.extend_from_slice(&[1, 2, 0xAB, 0xCD]); // one witness stack: 2 bytes
    raw.extend_from_slice(&[0, 0, 0, 0]);
    let parsed = tx::parse(&raw).unwrap();
    assert_eq!(parsed.outputs[0].value, 5);
    for cut in 0..raw.len() {
        assert!(tx::parse(&raw[..cut]).is_err(), "prefix of {cut} bytes");
    }
}

#[test]
fn a_length_longer_than_the_remaining_bytes_is_rejected() {
    // scriptPubKey length 0xfc but only 2 bytes follow.
    let mut raw = vec![2, 0, 0, 0, 1];
    raw.extend_from_slice(&[7u8; 32]);
    raw.extend_from_slice(&[0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
    raw.push(1);
    raw.extend_from_slice(&5u64.to_le_bytes());
    raw.push(0xfc);
    raw.extend_from_slice(&[0x51, 0x51]);
    raw.extend_from_slice(&[0, 0, 0, 0]);
    assert!(matches!(tx::parse(&raw), Err(BridgeError::Malformed(_))));
}

#[test]
fn compact_size_must_be_minimal_at_every_width_and_bounded() {
    let good = simple();
    let tail = &good[5..]; // input count (1) removed below; rebuild with each width
    let body = &tail[1..];
    // input count 1 encoded as fd 01 00 / fe 01 00 00 00 / ff 01 00.. : non-minimal.
    for prefix in [
        vec![0xfd, 1, 0],
        vec![0xfe, 1, 0, 0, 0],
        vec![0xff, 1, 0, 0, 0, 0, 0, 0, 0],
    ] {
        let mut raw = vec![2, 0, 0, 0];
        raw.extend_from_slice(&prefix);
        raw.extend_from_slice(body);
        assert_eq!(
            tx::parse(&raw),
            Err(BridgeError::Malformed("non-minimal CompactSize")),
            "{prefix:?}"
        );
    }
    // Boundary: 0xfd 0xfd 0x00 (253) is minimal for the width, so it gets
    // past the encoding check and fails on the bound or truncation instead.
    let mut raw = vec![2, 0, 0, 0, 0xfd, 0xfd, 0];
    raw.extend_from_slice(body);
    assert_ne!(
        tx::parse(&raw),
        Err(BridgeError::Malformed("non-minimal CompactSize"))
    );
    // A count past the item bound is refused before anything is reserved.
    let mut huge = vec![2, 0, 0, 0, 0xff];
    huge.extend_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(
        tx::parse(&huge),
        Err(BridgeError::Malformed("count out of range"))
    );
    let mut over = vec![2, 0, 0, 0, 0xfe];
    over.extend_from_slice(&100_001u32.to_le_bytes());
    assert_eq!(
        tx::parse(&over),
        Err(BridgeError::Malformed("count out of range"))
    );
}

#[test]
fn oversize_input_is_refused_before_it_is_walked() {
    let raw = vec![0u8; tx::MAX_TX_BYTES + 1];
    assert_eq!(
        tx::parse(&raw),
        Err(BridgeError::Malformed("transaction too large"))
    );
}

#[test]
fn op_return_32_accepts_only_the_exact_shape() {
    let payload = [0x5Au8; 32];
    let exact = [&[OP_RETURN, 32][..], &payload].concat();
    assert_eq!(tx::op_return_32(&exact), Some(payload));
    assert_eq!(tx::op_return_32(&[]), None);
    assert_eq!(tx::op_return_32(&[OP_RETURN]), None);
    assert_eq!(tx::op_return_32(&[OP_RETURN, 32]), None);
    assert_eq!(
        tx::op_return_32(&exact[..exact.len() - 1]),
        None,
        "31 bytes"
    );
    assert_eq!(
        tx::op_return_32(&[exact.clone(), vec![0]].concat()),
        None,
        "33 bytes"
    );
    let mut wrong_len_byte = exact.clone();
    wrong_len_byte[1] = 31;
    assert_eq!(tx::op_return_32(&wrong_len_byte), None);
    let mut pushdata1 = vec![OP_RETURN, 0x4c, 32];
    pushdata1.extend_from_slice(&payload);
    assert_eq!(tx::op_return_32(&pushdata1), None, "OP_PUSHDATA1 form");
    let mut wrong_op = exact;
    wrong_op[0] = 0x6b;
    assert_eq!(tx::op_return_32(&wrong_op), None);
}

#[derive(Clone, Debug)]
enum Op {
    Deposit(u64),
    Burn(u64),
    Pay { which: usize, short: bool },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u64..1_000_000).prop_map(Op::Deposit),
        (1u64..1_500_000).prop_map(Op::Burn),
        (0usize..8, any::<bool>()).prop_map(|(which, short)| Op::Pay { which, short }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// supply + owed == locked after every accepted or refused operation.
    #[test]
    fn wrapped_supply_plus_owed_always_equals_locked(ops in prop::collection::vec(op(), 1..14)) {
        let mut net = Net::new();
        for o in ops {
            match o {
                Op::Deposit(v) => {
                    let p = net.confirmed(&[(v, LOCK), (0, &recipient(ALICE))]);
                    prop_assert_eq!(net.bridge.mint(&p), Ok((ALICE, v)));
                }
                Op::Burn(v) => {
                    let before = net.bridge.balance(&ALICE);
                    let r = net.bridge.burn(ALICE, v, PAYOUT.to_vec());
                    prop_assert_eq!(r.is_ok(), v <= before);
                    if r.is_err() {
                        prop_assert_eq!(net.bridge.balance(&ALICE), before);
                    }
                }
                Op::Pay { which, short } => {
                    let ids: Vec<u64> = net.bridge.open_releases().keys().copied().collect();
                    if let Some(id) = ids.get(which % ids.len().max(1)).copied() {
                        let owed = net.bridge.open_releases()[&id].amount;
                        let paid = if short { owed - 1 } else { owed };
                        let p = net.confirmed(&[(paid, PAYOUT)]);
                        let r = net.bridge.confirm_release(id, &p);
                        prop_assert_eq!(r.is_ok(), !short);
                    }
                }
            }
            net.books_balance();
        }
    }
}
