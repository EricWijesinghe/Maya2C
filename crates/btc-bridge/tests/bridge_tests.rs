//! The Bitcoin bridge: real mainnet transactions parsed natively, and
//! lock/mint and burn/release over a regtest header chain, including the
//! brief's six-block reorg.

#![allow(clippy::unwrap_used)]

use maya_btc_bridge::tx::{self, OP_RETURN};
use maya_btc_bridge::{Bridge, BridgeError, InclusionProof};
use maya_btc_spv::{Header, HeaderChain, Params, merkle_root, prove, sha256d};
use serde_json::Value;

const LOCK: &[u8] = &[
    0x00, 0x14, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
    0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
];
const ALICE: [u8; 32] = [0xA1; 32];
const PAYOUT: &[u8] = &[
    0x00, 0x14, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB,
    0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB,
];
const CONFIRMATIONS: u32 = 6;

fn reversed(hex_display: &str) -> [u8; 32] {
    let mut b: [u8; 32] = hex::decode(hex_display).unwrap().try_into().unwrap();
    b.reverse();
    b
}

#[test]
fn real_mainnet_transactions_parse_to_their_txids_under_the_real_header() {
    let doc: Value = serde_json::from_str(include_str!("fixtures/mainnet_900000.json")).unwrap();
    let header = Header::decode(&hex::decode(doc["header"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(
        header.hash(),
        reversed(doc["block_hash"].as_str().unwrap()),
        "block 900,000's hash"
    );
    assert!(header.meets_own_target());
    let txids: Vec<[u8; 32]> = doc["txids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| reversed(t.as_str().unwrap()))
        .collect();
    assert_eq!(
        merkle_root(&txids),
        header.merkle_root,
        "all 1,562 txids rebuild the header's root"
    );
    for t in doc["transactions"].as_array().unwrap() {
        let parsed = tx::parse(&hex::decode(t["raw"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(
            parsed.txid,
            reversed(t["txid"].as_str().unwrap()),
            "txid of {}",
            t["txid"]
        );
        let index = usize::try_from(t["index"].as_u64().unwrap()).unwrap();
        assert!(
            prove(&txids, index)
                .unwrap()
                .verify_txid(&header.merkle_root, &parsed.txid)
        );
    }
}

/// A one-input legacy transaction paying `outputs`.
fn transaction(seed: u8, outputs: &[(u64, &[u8])]) -> Vec<u8> {
    let mut raw = vec![2, 0, 0, 0, 1];
    raw.extend_from_slice(&[seed; 32]);
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
}

impl Net {
    fn new() -> Self {
        let genesis = mine([0; 32], 1_600_000_000, sha256d(b"genesis"));
        let headers = HeaderChain::from_checkpoint(Params::REGTEST, 0, genesis);
        Self {
            bridge: Bridge::new(headers, LOCK.to_vec(), CONFIRMATIONS),
            tip: genesis.hash(),
            time: 1_600_000_000,
        }
    }

    /// Mines a block holding `txs` (after a filler coinbase) on `prev`;
    /// returns its hash and a proof for each transaction.
    fn block_on(&mut self, prev: [u8; 32], txs: &[Vec<u8>]) -> ([u8; 32], Vec<InclusionProof>) {
        let coinbase = transaction(0xC0, &[(1, b"\x51".as_slice())]);
        let all: Vec<Vec<u8>> = std::iter::once(coinbase)
            .chain(txs.iter().cloned())
            .collect();
        let txids: Vec<[u8; 32]> = all.iter().map(|t| tx::parse(t).unwrap().txid).collect();
        self.time += 600;
        let header = mine(prev, self.time, merkle_root(&txids));
        self.bridge.headers.add(header).unwrap();
        let hash = header.hash();
        let proofs = (1..all.len())
            .map(|i| InclusionProof {
                block: hash,
                raw_tx: all[i].clone(),
                merkle: prove(&txids, i).unwrap(),
            })
            .collect();
        (hash, proofs)
    }

    fn block(&mut self, txs: &[Vec<u8>]) -> Vec<InclusionProof> {
        let (hash, proofs) = self.block_on(self.tip, txs);
        self.tip = hash;
        proofs
    }

    fn bury(&mut self, blocks: u32) {
        for _ in 0..blocks {
            self.block(&[]);
        }
    }

    /// supply + owed == locked: every wrapped satoshi is backed by one the
    /// bridge has seen locked and not yet seen released.
    fn books_balance(&self) {
        assert_eq!(
            self.bridge.supply() + self.bridge.owed(),
            self.bridge.locked()
        );
    }
}

#[test]
fn a_deposit_mints_once_at_depth_and_not_before() {
    let mut net = Net::new();
    let deposit = transaction(1, &[(50_000, LOCK), (0, &recipient(ALICE))]);
    let proof = net.block(&[deposit]).remove(0);
    net.bury(CONFIRMATIONS - 2);
    assert_eq!(
        net.bridge.mint(&proof),
        Err(BridgeError::TooShallow {
            have: CONFIRMATIONS - 1,
            need: CONFIRMATIONS
        })
    );
    net.bury(1);
    assert_eq!(net.bridge.mint(&proof), Ok((ALICE, 50_000)));
    assert_eq!(
        net.bridge.mint(&proof),
        Err(BridgeError::AlreadyUsed),
        "one output, one mint"
    );
    net.books_balance();

    // A relayer cannot move a transaction to another block, name no
    // recipient, pay another script, or pass off a 64-byte forgery.
    let other = net
        .block(&[transaction(9, &[(1, LOCK), (0, &recipient(ALICE))])])
        .remove(0);
    net.bury(CONFIRMATIONS);
    let moved = InclusionProof {
        block: other.block,
        ..proof.clone()
    };
    assert_eq!(net.bridge.mint(&moved), Err(BridgeError::NotIncluded));
    let anonymous = net.block(&[transaction(2, &[(10, LOCK)])]).remove(0);
    let elsewhere = net
        .block(&[transaction(3, &[(10, PAYOUT), (0, &recipient(ALICE))])])
        .remove(0);
    net.bury(CONFIRMATIONS);
    assert_eq!(net.bridge.mint(&anonymous), Err(BridgeError::NoRecipient));
    assert_eq!(net.bridge.mint(&elsewhere), Err(BridgeError::NoDeposit));
    let forged = InclusionProof {
        raw_tx: vec![0; 64],
        ..anonymous
    };
    assert!(matches!(
        net.bridge.mint(&forged),
        Err(BridgeError::Malformed(_))
    ));
    net.books_balance();
}

#[test]
fn a_six_block_reorg_before_credit_leaves_the_bridge_whole() {
    let mut net = Net::new();
    let fork_point = net.tip;
    let deposit = transaction(4, &[(70_000, LOCK), (0, &recipient(ALICE))]);
    let proof = net.block(&[deposit]).remove(0);
    net.bury(CONFIRMATIONS - 2); // five deep: not yet credited
    assert!(matches!(
        net.bridge.mint(&proof),
        Err(BridgeError::TooShallow { .. })
    ));

    // An attacker mines six blocks from before the deposit: more work, and
    // the deposit's block leaves the best chain.
    let mut attack = fork_point;
    for _ in 0..CONFIRMATIONS {
        attack = net.block_on(attack, &[]).0;
    }
    assert_eq!(net.bridge.headers.tip(), attack, "the longer fork wins");
    assert_eq!(
        net.bridge.mint(&proof),
        Err(BridgeError::TooShallow {
            have: 0,
            need: CONFIRMATIONS
        })
    );
    assert_eq!(
        net.bridge.supply(),
        0,
        "nothing was minted against a reversed deposit"
    );

    // The honest chain comes back with more work, and the deposit mints.
    net.bury(3);
    assert_eq!(net.bridge.mint(&proof), Ok((ALICE, 70_000)));
    net.books_balance();
}

#[test]
fn a_release_closes_only_on_a_proven_payout() {
    let mut net = Net::new();
    let proof = net
        .block(&[transaction(5, &[(90_000, LOCK), (0, &recipient(ALICE))])])
        .remove(0);
    net.bury(CONFIRMATIONS);
    net.bridge.mint(&proof).unwrap();
    assert_eq!(
        net.bridge.burn(ALICE, 90_001, PAYOUT.to_vec()),
        Err(BridgeError::Insufficient)
    );
    let id = net.bridge.burn(ALICE, 60_000, PAYOUT.to_vec()).unwrap();
    net.books_balance();
    assert_eq!(net.bridge.owed(), 60_000);

    // Custody underpays first, then pays in full.
    let short = net.block(&[transaction(6, &[(59_999, PAYOUT)])]).remove(0);
    let full = net.block(&[transaction(7, &[(60_000, PAYOUT)])]).remove(0);
    assert!(matches!(
        net.bridge.confirm_release(id, &full),
        Err(BridgeError::TooShallow { .. })
    ));
    net.bury(CONFIRMATIONS);
    assert_eq!(
        net.bridge.confirm_release(id, &short),
        Err(BridgeError::DoesNotPay)
    );
    net.bridge.confirm_release(id, &full).unwrap();
    assert_eq!(
        net.bridge.confirm_release(id, &full),
        Err(BridgeError::UnknownRelease(id))
    );
    assert_eq!(
        (net.bridge.locked(), net.bridge.supply(), net.bridge.owed()),
        (30_000, 30_000, 0)
    );

    // One payout closes one release.
    let second = net.bridge.burn(ALICE, 30_000, PAYOUT.to_vec()).unwrap();
    assert_eq!(
        net.bridge.confirm_release(second, &full),
        Err(BridgeError::AlreadyUsed)
    );
    net.books_balance();
}

#[test]
fn malformed_transactions_are_refused_not_misread() {
    let good = transaction(8, &[(1, LOCK), (0, &recipient(ALICE))]);
    assert!(tx::parse(&good).is_ok());
    for cut in 0..good.len() {
        assert!(tx::parse(&good[..cut]).is_err(), "prefix of {cut} bytes");
    }
    assert!(
        tx::parse(&[good.clone(), vec![0]].concat()).is_err(),
        "trailing byte"
    );
    let mut no_inputs = good.clone();
    no_inputs[4] = 0;
    no_inputs.insert(5, 2); // segwit marker then an unknown flag
    assert!(tx::parse(&no_inputs).is_err());
    let mut non_minimal = vec![2, 0, 0, 0, 0xfd, 1, 0];
    non_minimal.extend_from_slice(&good[5..]);
    assert!(tx::parse(&non_minimal).is_err(), "non-minimal CompactSize");
}

#[test]
fn one_transaction_cannot_both_mint_and_release_or_name_two_recipients() {
    let mut net = Net::new();
    let proof = net
        .block(&[transaction(10, &[(100_000, LOCK), (0, &recipient(ALICE))])])
        .remove(0);
    net.bury(CONFIRMATIONS);
    net.bridge.mint(&proof).unwrap();
    // A release to the lock script: its payout would also be a deposit.
    assert_eq!(
        net.bridge.burn(ALICE, 100_000, LOCK.to_vec()),
        Err(BridgeError::ReleaseToLock)
    );

    // A transaction paying both the lock and a release's script is one or
    // the other, never both.
    let id = net.bridge.burn(ALICE, 40_000, PAYOUT.to_vec()).unwrap();
    let both = net
        .block(&[transaction(
            11,
            &[(40_000, PAYOUT), (40_000, LOCK), (0, &recipient(ALICE))],
        )])
        .remove(0);
    net.bury(CONFIRMATIONS);
    net.bridge.mint(&both).unwrap();
    assert_eq!(
        net.bridge.confirm_release(id, &both),
        Err(BridgeError::AlreadyUsed)
    );
    net.books_balance();

    // Two recipients: refused, rather than whichever is read first.
    let two = net
        .block(&[transaction(
            12,
            &[
                (5, LOCK),
                (0, &recipient(ALICE)),
                (0, &recipient([0xEE; 32])),
            ],
        )])
        .remove(0);
    net.bury(CONFIRMATIONS);
    assert_eq!(net.bridge.mint(&two), Err(BridgeError::NoRecipient));
    net.books_balance();
}
