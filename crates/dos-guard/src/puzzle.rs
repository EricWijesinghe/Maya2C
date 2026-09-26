//! Client puzzles: find `nonce` with `blake3(challenge ‖ nonce)` having at
//! least `bits` leading zero bits. Solving costs about 2^bits hashes;
//! checking costs one. The node sets `bits` to 0 normally and raises it only
//! when handshake budgets are exhausted, so honest peers pay only under attack.

/// Leading zero bits of a hash.
fn leading_zero_bits(h: &[u8; 32]) -> u32 {
    let mut bits = 0;
    for b in h {
        if *b == 0 {
            bits += 8;
        } else {
            bits += b.leading_zeros();
            break;
        }
    }
    bits
}

fn digest(challenge: &[u8; 32], nonce: u64) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("maya2c handshake puzzle v1");
    h.update(challenge);
    h.update(&nonce.to_le_bytes());
    *h.finalize().as_bytes()
}

/// Whether `nonce` solves `challenge` at `bits`.
#[must_use]
pub fn check(challenge: &[u8; 32], nonce: u64, bits: u32) -> bool {
    leading_zero_bits(&digest(challenge, nonce)) >= bits
}

/// Solves a puzzle (the client's side). Returns the nonce and the attempts.
#[must_use]
pub fn solve(challenge: &[u8; 32], bits: u32) -> (u64, u64) {
    let mut nonce = 0u64;
    loop {
        if check(challenge, nonce, bits) {
            return (nonce, nonce + 1);
        }
        nonce += 1;
    }
}
