//! Gap tests (2026-11-10 scan): retarget against Bitcoin Core's own vectors,
//! the retarget wiring inside the chain, median-time-past boundaries,
//! duplicates, chainwork, `U256` carries, and merkle branches with their
//! siblings tampered.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use maya_btc_spv::{
    Header, HeaderChain, HeaderError, MerkleProof, Params, U256, compact_from_target, merkle_root,
    prove, sha256d, target_from_compact, work_from_target,
};

/// Bitcoin Core `src/test/pow_tests.cpp`, the `get_next_work*` cases,
/// checked against the file at
/// <https://github.com/bitcoin/bitcoin/blob/316ea3280e9981453457bee3bf1e1560936adb85/src/test/pow_tests.cpp>
/// (latest commit touching it on 2026-11-10). Each entry is (time of the
/// period's first block, time of its last block, last bits, expected next
/// bits, Core's test name).
const CORE_RETARGETS: [(u32, u32, u32, u32, &str); 6] = [
    (
        1_261_130_161,
        1_262_152_739,
        0x1d00_ffff,
        0x1d00_d86a,
        "get_next_work",
    ),
    (
        1_231_006_505,
        1_233_061_996,
        0x1d00_ffff,
        0x1d00_ffff,
        "get_next_work_pow_limit",
    ),
    (
        1_279_008_237,
        1_279_297_671,
        0x1c05_a3f4,
        0x1c01_68fd,
        "get_next_work_lower_limit_actual",
    ),
    // first = last - timespan / 4, with a mantissa that is not a multiple of four.
    (
        1_279_297_671 - 1_209_600 / 4,
        1_279_297_671,
        0x1c05_a3f5,
        0x1c01_68fd,
        "get_next_work_lower_limit_rounding",
    ),
    // The "first" block is stamped after the last one.
    (
        1_279_297_671,
        1_279_008_237,
        0x1c05_a3f4,
        0x1c01_68fd,
        "get_next_work_negative_actual",
    ),
    // Core notes the first time here is "not an actual block time".
    (
        1_263_163_443,
        1_269_211_443,
        0x1c38_7f6f,
        0x1d00_e1fd,
        "get_next_work_upper_limit_actual",
    ),
];

#[test]
fn retarget_matches_bitcoin_cores_mainnet_vectors() {
    for (first, last, bits, expected, what) in CORE_RETARGETS {
        assert_eq!(
            Params::MAINNET.retarget(first, last, bits),
            Some(expected),
            "{what}"
        );
    }
}

#[test]
fn retarget_clamp_boundaries_are_exact() {
    let p = Params::MAINNET;
    let quarter = p.target_timespan / 4;
    let bits = 0x1c05_a3f4;
    // One second under the quarter clamp equals the clamp; one over does not.
    let at = p.retarget(0, quarter, bits).unwrap();
    assert_eq!(p.retarget(0, quarter - 1, bits), Some(at));
    assert_ne!(p.retarget(0, quarter + 600, bits), Some(at));
    // A negative span (last block stamped before the first) clamps up.
    assert_eq!(p.retarget(1_000_000, 0, bits), Some(at));
    // Same on the slow side, at the 4x ceiling.
    let slow = p.retarget(0, p.target_timespan * 4, 0x1b04_04cb).unwrap();
    assert_eq!(
        p.retarget(0, p.target_timespan * 40, 0x1b04_04cb),
        Some(slow)
    );
    // An invalid previous encoding yields no answer instead of a panic.
    assert_eq!(p.retarget(0, p.target_timespan, 0xff12_3456), None);
}

fn mine(prev: [u8; 32], time: u32, bits: u32, tag: u8) -> Header {
    let mut h = Header {
        version: 0x2000_0000,
        prev,
        merkle_root: sha256d(&[tag]),
        time,
        bits,
        nonce: 0,
    };
    while !h.meets_own_target() {
        h.nonce += 1;
    }
    h
}

const EASY: u32 = 0x207f_ffff;
const T0: u32 = 1_600_000_000;

fn regtest() -> (HeaderChain, Header) {
    let g = mine([0; 32], T0, EASY, 0);
    (HeaderChain::from_checkpoint(Params::REGTEST, 0, g), g)
}

/// Adds a header at `time` on `prev`; returns the header.
fn push(chain: &mut HeaderChain, prev: &Header, time: u32, tag: u8) -> Header {
    let h = mine(prev.hash(), time, EASY, tag);
    chain.add(h).unwrap();
    h
}

#[test]
fn median_time_past_is_a_strict_boundary_over_the_last_eleven() {
    let (mut chain, mut tip) = regtest();
    // Heights 1..=11 at T0 + 10*i. The last eleven are heights 1..=11, so
    // the median is height 6: T0 + 60. Genesis (T0) has already left the window.
    for i in 1..=11u32 {
        tip = push(&mut chain, &tip, T0 + 10 * i, u8::try_from(i).unwrap());
    }
    let at_median = mine(tip.hash(), T0 + 60, EASY, 100);
    assert_eq!(chain.add(at_median), Err(HeaderError::TimeTooOld));
    let below = mine(tip.hash(), T0 + 59, EASY, 101);
    assert_eq!(chain.add(below), Err(HeaderError::TimeTooOld));
    let above = mine(tip.hash(), T0 + 61, EASY, 102);
    assert_eq!(chain.add(above), Ok(true));
}

#[test]
fn median_time_past_uses_fewer_ancestors_near_the_checkpoint() {
    // Ancestors are [genesis, b1, b2]: the median is b1's time.
    let (mut chain, g) = regtest();
    let b1 = push(&mut chain, &g, T0 + 100, 1);
    let b2 = push(&mut chain, &b1, T0 + 200, 2);
    assert_eq!(
        chain.add(mine(b2.hash(), T0 + 100, EASY, 3)),
        Err(HeaderError::TimeTooOld)
    );
    assert_eq!(chain.add(mine(b2.hash(), T0 + 101, EASY, 4)), Ok(true));
}

#[test]
fn a_timestamp_below_its_parent_is_fine_while_it_beats_the_median() {
    let (mut chain, g) = regtest();
    let b1 = push(&mut chain, &g, T0 + 100, 1);
    let b2 = push(&mut chain, &b1, T0 + 200, 2);
    let b3 = push(&mut chain, &b2, T0 + 150, 3);
    let b4 = push(&mut chain, &b3, T0 + 160, 4);
    // Window [T0, +100, +200, +150, +160]: median T0 + 150.
    assert_eq!(
        chain.add(mine(b4.hash(), T0 + 150, EASY, 5)),
        Err(HeaderError::TimeTooOld)
    );
    assert_eq!(chain.add(mine(b4.hash(), T0 + 151, EASY, 6)), Ok(true));
}

#[test]
fn a_header_held_twice_is_a_duplicate_and_changes_nothing() {
    let (mut chain, g) = regtest();
    let b1 = push(&mut chain, &g, T0 + 100, 1);
    let (tip, work, height) = (chain.tip(), chain.chainwork(), chain.height());
    assert_eq!(chain.add(b1), Err(HeaderError::Duplicate));
    assert_eq!(chain.add(g), Err(HeaderError::Duplicate), "the checkpoint");
    assert_eq!(
        (chain.tip(), chain.chainwork(), chain.height()),
        (tip, work, height)
    );
}

#[test]
fn chainwork_grows_by_exactly_the_work_of_each_header_and_ties_keep_the_tip() {
    let (mut chain, g) = regtest();
    let unit = work_from_target(target_from_compact(EASY).unwrap());
    assert_eq!(chain.chainwork(), unit);
    let b1 = push(&mut chain, &g, T0 + 100, 1);
    let b2 = push(&mut chain, &b1, T0 + 200, 2);
    assert_eq!(chain.chainwork(), unit.checked_mul_u64(3).unwrap());
    // An equal-work sibling of b2 does not displace the tip.
    let sibling = mine(b1.hash(), T0 + 201, EASY, 9);
    assert_eq!(chain.add(sibling), Ok(false));
    assert_eq!(chain.tip(), b2.hash());
    assert_eq!(chain.confirmations(&sibling.hash()), 0);
    // Mainnet genesis work is the textbook 2^32 + 2^16 + 1 per block.
    let mainnet = work_from_target(target_from_compact(0x1d00_ffff).unwrap());
    assert_eq!(mainnet, U256::from_u64(0x1_0001_0001));
}

/// A 4-block "year" so a retarget is reachable by mining. The limit is the
/// widest that leaves room for the 4x timespan product inside 256 bits, and
/// still mines in about 256 hashes.
const WIDE: u32 = 0x2000_ffff;
const TINY: Params = Params {
    pow_limit_bits: WIDE,
    interval: 4,
    target_timespan: 40,
    no_retargeting: false,
};

#[test]
fn the_chain_demands_the_retargeted_bits_at_the_boundary_and_not_between() {
    let g = mine([0; 32], T0, WIDE, 0);
    let mut chain = HeaderChain::from_checkpoint(TINY, 0, g);
    let mut tip = g;
    // Heights 1..=3 arrive 1 s apart: the 3 s span is under the floor of
    // 10 s (timespan / 4), so the clamp applies and the target quarters.
    for i in 1..=3u32 {
        tip = mine(tip.hash(), T0 + i, WIDE, u8::try_from(i).unwrap());
        chain.add(tip).unwrap();
    }
    let want = TINY.retarget(T0, T0 + 3, WIDE).unwrap();
    assert_eq!(
        want,
        compact_from_target(target_from_compact(WIDE).unwrap().div_u64(4))
    );
    let unchanged = mine(tip.hash(), T0 + 4, WIDE, 50);
    assert_eq!(
        chain.add(unchanged),
        Err(HeaderError::WrongDifficulty {
            expected: want,
            found: WIDE
        })
    );
    let harder = mine(tip.hash(), T0 + 4, want, 51);
    assert_eq!(chain.add(harder), Ok(true));
    // Off the boundary the bits carry over: changing them is refused.
    let drift = mine(harder.hash(), T0 + 5, WIDE, 52);
    assert_eq!(
        chain.add(drift),
        Err(HeaderError::WrongDifficulty {
            expected: want,
            found: WIDE
        })
    );
}

#[test]
fn a_slow_period_at_the_pow_limit_keeps_the_limit() {
    let g = mine([0; 32], T0, WIDE, 0);
    let mut chain = HeaderChain::from_checkpoint(TINY, 0, g);
    let mut tip = g;
    for i in 1..=3u32 {
        tip = mine(tip.hash(), T0 + 100 * i, WIDE, u8::try_from(i).unwrap());
        chain.add(tip).unwrap();
    }
    // 300 s clamps to 160 s: the target would grow 4x, past the limit.
    assert_eq!(chain.add(mine(tip.hash(), T0 + 400, WIDE, 9)), Ok(true));
}

#[test]
fn a_retarget_too_wide_for_256_bits_fails_closed() {
    // EASY (regtest's limit) times a 2400 s timespan overflows U256: the
    // chain must refuse the boundary header, not wrap into an easy target.
    // Unreachable on mainnet, whose 224-bit limit leaves 32 bits of room.
    let params = Params {
        pow_limit_bits: EASY,
        interval: 4,
        target_timespan: 2_400,
        no_retargeting: false,
    };
    assert_eq!(params.retarget(T0, T0 + 300, EASY), None);
    let g = mine([0; 32], T0, EASY, 0);
    let mut chain = HeaderChain::from_checkpoint(params, 0, g);
    let mut tip = g;
    for i in 1..=3u32 {
        tip = push(&mut chain, &tip, T0 + 100 * i, u8::try_from(i).unwrap());
    }
    assert_eq!(
        chain.add(mine(tip.hash(), T0 + 400, EASY, 9)),
        Err(HeaderError::WrongDifficulty {
            expected: 0,
            found: EASY
        })
    );
}

#[test]
fn a_retarget_whose_period_starts_before_the_checkpoint_is_refused() {
    let g = mine([0; 32], T0, WIDE, 0);
    let mut chain = HeaderChain::from_checkpoint(TINY, 1, g);
    let mut tip = g;
    for i in 1..=2u32 {
        tip = mine(tip.hash(), T0 + i, WIDE, u8::try_from(i).unwrap());
        chain.add(tip).unwrap();
    }
    // Height 4 needs the header at height 0, which this chain never saw.
    assert_eq!(
        chain.add(mine(tip.hash(), T0 + 3, WIDE, 9)),
        Err(HeaderError::RetargetBeforeCheckpoint)
    );
}

#[test]
fn u256_carries_borrows_and_bit_lengths_cross_limb_boundaries() {
    let m = u64::MAX;
    assert_eq!(
        U256([m, m, m, 0]).checked_add(U256::from_u64(1)),
        Some(U256([0, 0, 0, 1]))
    );
    assert_eq!(U256([m, m, m, m]).checked_add(U256::ZERO), Some(U256::MAX));
    assert_eq!(
        U256([0, 0, 0, 1]).wrapping_sub(U256::from_u64(1)),
        U256([m, m, m, 0])
    );
    assert_eq!(U256::ZERO.bits(), 0);
    assert_eq!(U256([m, 0, 0, 0]).bits(), 64);
    assert_eq!(U256([0, 1, 0, 0]).bits(), 65);
    assert_eq!(U256([0, 0, 1, 0]).bits(), 129);
    assert_eq!(U256([0, 0, 0, 1 << 63]).bits(), 256);
    // A product that carries across every limb, and one that overflows.
    assert_eq!(
        U256([m, m, m, 0]).checked_mul_u64(2),
        Some(U256([m - 1, m, m, 1]))
    );
    assert_eq!(U256([0, 0, 0, 1 << 63]).checked_mul_u64(2), None);
    assert_eq!(U256::MAX.checked_mul_u64(1), Some(U256::MAX));
    assert_eq!(U256::MAX.checked_mul_u64(0), Some(U256::ZERO));
    assert!(U256([0, 0, 0, 1]) > U256([m, m, m, 0]));
}

#[test]
fn compact_targets_round_trip_at_every_shift_distance() {
    // Exponents 4..=32 exercise limb-crossing shifts (8*(e-3) from 8 to 232).
    for exponent in 4u32..=32 {
        let bits = exponent << 24 | 0x0012_3456;
        let t = target_from_compact(bits).unwrap_or_else(|| panic!("{bits:#x}"));
        assert_eq!(compact_from_target(t), bits, "{bits:#x}");
        assert_eq!(t.bits(), 8 * (exponent - 3) + 21, "{bits:#x}");
    }
    // The widest encodable target: 0xffff << 240 fills bit 255.
    let top = target_from_compact(0x2100_ffff).unwrap();
    assert_eq!(top.bits(), 256);
    assert_eq!(compact_from_target(top), 0x2100_ffff);
    assert_eq!(target_from_compact(0x2200_ffff), None, "past 256 bits");
}

fn tampered_cases() -> (Vec<[u8; 32]>, [u8; 32]) {
    let ids: Vec<[u8; 32]> = (0..7u8).map(|i| sha256d(&[i])).collect();
    let root = merkle_root(&ids);
    (ids, root)
}

#[test]
fn a_swapped_or_altered_merkle_branch_never_verifies() {
    let (ids, root) = tampered_cases();
    for i in 0..ids.len() {
        let good = prove(&ids, i).unwrap();
        assert!(good.verify_txid(&root, &ids[i]));
        // Swap each pair of distinct siblings.
        for a in 0..good.siblings.len() {
            for b in a + 1..good.siblings.len() {
                if good.siblings[a] != good.siblings[b] {
                    let mut p = good.clone();
                    p.siblings.swap(a, b);
                    assert!(!p.verify_txid(&root, &ids[i]), "i={i} swap {a},{b}");
                }
            }
        }
        // Flip a bit in each sibling.
        for a in 0..good.siblings.len() {
            let mut p = good.clone();
            p.siblings[a][0] ^= 1;
            assert!(!p.verify_txid(&root, &ids[i]), "i={i} flip {a}");
        }
        // Wrong leaf, wrong root, and a truncated or extended branch.
        assert!(!good.verify_txid(&root, &ids[(i + 1) % ids.len()]));
        assert!(!good.verify_txid(&[0u8; 32], &ids[i]));
        let mut short = good.clone();
        short.siblings.pop();
        assert!(!short.verify_txid(&root, &ids[i]));
        let mut long = good.clone();
        long.siblings.push([0; 32]);
        assert!(!long.verify_txid(&root, &ids[i]));
        // The same siblings claimed at the neighbouring index: left/right
        // order flips, so the root must not match.
        let neighbour = MerkleProof {
            index: u32::try_from(i ^ 1).unwrap(),
            ..good.clone()
        };
        if i ^ 1 < ids.len() {
            assert!(!neighbour.verify_txid(&root, &ids[i]), "i={i}");
        }
    }
}

#[test]
fn an_out_of_range_index_or_zero_count_never_verifies() {
    let (ids, root) = tampered_cases();
    let good = prove(&ids, 3).unwrap();
    let mut p = good.clone();
    p.index = u32::try_from(ids.len()).unwrap();
    assert!(!p.verify_txid(&root, &ids[3]));
    let mut zero = good;
    zero.tx_count = 0;
    assert!(!zero.verify_txid(&root, &ids[3]));
    assert!(prove(&ids, ids.len()).is_none());
}
