//! Real mainnet headers, the retarget rule, and a six-block reorg.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_possible_truncation, clippy::needless_range_loop)]

use maya_btc_spv::{
    Header, HeaderChain, HeaderError, Params, merkle_root, prove, sha256d, target_from_compact,
};

/// Parses a display-order (reversed) hex hash into internal order.
fn display(hex: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    out.reverse();
    out
}

/// Bitcoin mainnet blocks 0, 1 and 2, as every explorer shows them.
fn mainnet_headers() -> [(Header, [u8; 32]); 3] {
    let genesis = Header {
        version: 1,
        prev: [0; 32],
        merkle_root: display("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"),
        time: 1_231_006_505,
        bits: 0x1d00_ffff,
        nonce: 2_083_236_893,
    };
    let h0 = display("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f");
    let block1 = Header {
        version: 1,
        prev: h0,
        merkle_root: display("0e3e2357e806b6cdb1f70b54c3a3a17b6714ee1f0e68bebb44a74b1efd512098"),
        time: 1_231_469_665,
        bits: 0x1d00_ffff,
        nonce: 2_573_394_689,
    };
    let h1 = display("00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048");
    let block2 = Header {
        version: 1,
        prev: h1,
        merkle_root: display("9b0fc92260312ce44e74ef369f5c66bbb85848f2eddd5a7a1cde251e54ccfdd5"),
        time: 1_231_469_744,
        bits: 0x1d00_ffff,
        nonce: 1_639_830_024,
    };
    let h2 = display("000000006a625f06636b8bb6ac7b960a8d03705d1ace08b1a19da3fdcc99ddbd");
    [(genesis, h0), (block1, h1), (block2, h2)]
}

#[test]
fn real_mainnet_headers_hash_meet_their_targets_and_chain() {
    let headers = mainnet_headers();
    for (h, expected) in &headers {
        assert_eq!(h.hash(), *expected);
        assert!(h.meets_own_target());
        assert_eq!(Header::decode(&h.encode()), Some(*h));
    }
    let mut chain = HeaderChain::from_checkpoint(Params::MAINNET, 0, headers[0].0);
    assert_eq!(chain.add(headers[1].0), Ok(true));
    assert_eq!(chain.add(headers[2].0), Ok(true));
    assert_eq!(chain.height(), 2);
    assert_eq!(chain.confirmations(&headers[0].1), 3);
    // Genesis has one transaction, so its merkle root is that txid.
    assert_eq!(merkle_root(&[headers[0].0.merkle_root]), headers[0].0.merkle_root);
}

#[test]
fn a_tampered_mainnet_header_is_refused() {
    let headers = mainnet_headers();
    let mut chain = HeaderChain::from_checkpoint(Params::MAINNET, 0, headers[0].0);
    let mut forged = headers[1].0;
    forged.nonce ^= 1;
    assert_eq!(chain.add(forged), Err(HeaderError::InsufficientWork));
    let mut easier = headers[1].0;
    easier.bits = 0x207f_ffff;
    assert!(matches!(chain.add(easier), Err(HeaderError::WrongDifficulty { .. })));
    let mut orphan = headers[2].0;
    orphan.prev = [9; 32];
    assert_eq!(chain.add(orphan), Err(HeaderError::UnknownParent));
}

#[test]
fn the_retarget_rule_clamps_to_four_times_and_the_pow_limit() {
    let p = Params::MAINNET;
    let two_weeks = p.target_timespan;
    // On schedule: unchanged.
    assert_eq!(p.retarget(0, two_weeks, 0x1b04_04cb), Some(0x1b04_04cb));
    // Four times too fast (or faster): target divided by four, no further.
    let fast = p.retarget(0, two_weeks / 10, 0x1b04_04cb).unwrap();
    assert_eq!(fast, p.retarget(0, two_weeks / 4, 0x1b04_04cb).unwrap());
    let t0 = target_from_compact(0x1b04_04cb).unwrap();
    // Compact encoding keeps three bytes of mantissa, so compare encodings.
    assert_eq!(fast, maya_btc_spv::compact_from_target(t0.div_u64(4)));
    // Slow at the easiest difficulty: capped at the limit.
    assert_eq!(p.retarget(0, two_weeks * 10, 0x1d00_ffff), Some(0x1d00_ffff));
    // A real-chain retarget vector is not included: it needs the timestamps
    // of blocks 30240 and 32255, which this test would have to take on
    // trust. The rule is checked against its definition above instead.
}

/// Mines a regtest header (target 0x207fffff: about one hash in two).
fn mine(prev: [u8; 32], time: u32, tag: u8) -> Header {
    let mut h = Header {
        version: 0x2000_0000,
        prev,
        merkle_root: sha256d(&[tag]),
        time,
        bits: 0x207f_ffff,
        nonce: 0,
    };
    while !h.meets_own_target() {
        h.nonce += 1;
    }
    h
}

fn extend(chain: &mut HeaderChain, from: [u8; 32], start_time: u32, n: u32, tag: u8) -> Vec<[u8; 32]> {
    let mut prev = from;
    let mut hashes = Vec::new();
    for i in 0..n {
        let h = mine(prev, start_time + 600 * (i + 1), tag.wrapping_add(i as u8));
        chain.add(h).unwrap();
        prev = h.hash();
        hashes.push(prev);
    }
    hashes
}

#[test]
fn a_six_block_reorg_displaces_a_shallow_deposit_and_not_a_deep_one() {
    let genesis = mine([0; 32], 1_600_000_000, 0);
    let mut chain = HeaderChain::from_checkpoint(Params::REGTEST, 0, genesis);
    let honest = extend(&mut chain, genesis.hash(), 1_600_000_000, 10, 1);
    // A deposit in honest block 5 (height 5) is 6 deep at height 10.
    let deposit_block = honest[4];
    assert_eq!(chain.confirmations(&deposit_block), 6);
    // An attacker forks at height 4 and mines 7 blocks: height 11 > 10.
    let fork_point = honest[3];
    let attack = extend(&mut chain, fork_point, 1_600_000_000 + 2_400, 7, 100);
    assert_eq!(chain.tip(), *attack.last().unwrap(), "more work wins");
    assert_eq!(chain.confirmations(&deposit_block), 0, "the 6-deep deposit is gone");
    // A bridge that waited for 8 confirmations would not yet have credited it,
    // so it loses nothing: depth is the policy, and it is stated.

    // The honest chain comes back with more work and the deposit returns.
    let more = extend(&mut chain, *honest.last().unwrap(), 1_600_000_000 + 6_000, 3, 200);
    assert_eq!(chain.tip(), *more.last().unwrap());
    assert_eq!(chain.confirmations(&deposit_block), 9);
}

#[test]
fn merkle_proofs_hold_for_every_position_and_refuse_the_known_tricks() {
    for n in 1..20usize {
        let txs: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8; 100 + i]).collect();
        let ids: Vec<[u8; 32]> = txs.iter().map(|t| sha256d(t)).collect();
        let root = merkle_root(&ids);
        for i in 0..n {
            let p = prove(&ids, i).unwrap();
            assert!(p.verify_tx(&root, &txs[i]), "n={n} i={i}");
            assert!(!p.verify_tx(&root, b"forged transaction bytes"));
        }
    }
    // A 64-byte "transaction" is refused outright.
    let ids: Vec<[u8; 32]> = (0..4u8).map(|i| sha256d(&[i])).collect();
    let root = merkle_root(&ids);
    let p = prove(&ids, 0).unwrap();
    assert!(!p.verify_tx(&root, &[0u8; 64]));
}
