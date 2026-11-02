//! Fuzzes the pure proof-of-work target rules and the prune-horizon predicate.
//!
//! Header validation is `digest <= target`, and the target is built from a
//! leading-zero-bit count. The law that ties them is what a header check
//! relies on, so the fuzzer asserts it for arbitrary digests and counts. The
//! prune predicate is checked for monotonicity: a block refused at one horizon
//! must stay refused at any higher one.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::consensus::chain::below_prune_horizon;
use custom_l1_node::crypto::pow::{leading_zero_bits, meets_target, target_from_leading_zero_bits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 32 + 32 + 8 + 8 + 8 {
        return;
    }
    let digest: [u8; 32] = data[..32].try_into().expect("sized");
    let target: [u8; 32] = data[32..64].try_into().expect("sized");
    let bits = u32::from_le_bytes(data[64..68].try_into().expect("sized")) % 300;
    let height = u64::from_le_bytes(data[68..76].try_into().expect("sized"));
    let horizon = u64::from_le_bytes(data[76..84].try_into().expect("sized"));

    // A digest meets a target exactly when it is at most the target as a
    // big-endian integer, which is lexicographic order on 32 bytes.
    assert_eq!(meets_target(&digest, &target), digest <= target);

    // The count-built target accepts a digest exactly when it has at least
    // that many leading zeros. Above 256 bits only the zero digest is left.
    let built = target_from_leading_zero_bits(bits);
    let needed = bits.min(256);
    assert_eq!(
        meets_target(&digest, &built),
        leading_zero_bits(&digest) >= needed,
        "target law broken at {bits} bits"
    );

    // One more required zero bit can only shrink the target.
    let stricter = target_from_leading_zero_bits(bits + 1);
    assert!(stricter <= built, "a stricter count raised the target");

    // Refused at a horizon implies refused at any higher one.
    if below_prune_horizon(height, horizon) {
        assert!(below_prune_horizon(height, horizon.saturating_add(1)));
    }
    assert_eq!(below_prune_horizon(height, horizon), height <= horizon);
});
