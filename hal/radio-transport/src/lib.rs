//! Off-grid radio transport: moving `Maya2C` headers where there is no IP.
//!
//! ## What this carries, and what it does not
//!
//! **Headers and SPV proofs. Not transactions.**
//!
//! That is arithmetic rather than preference. Every `Maya2C` signature is a
//! hybrid pair — ML-DSA-65 plus SLH-DSA, `HYBRID_SIGNATURE_LENGTH` = 11,165
//! bytes — and both halves must verify, so there is no smaller signature to
//! send. Those bytes are also incompressible by construction: FIPS 204 already
//! bit-packs `z`, `h` and `c` at their information-theoretic width, and
//! SLH-DSA is a tree of hash outputs. A compressor applied to either produces
//! output larger than its input.
//!
//! Against that, the link:
//!
//! | | |
//! |---|---|
//! | `BlockHeader` | 144 bytes — one frame at any spreading factor |
//! | `AccountProof` | ~600 B – 1 KB — three to five frames |
//! | One transaction | ~13 KB — 60 frames at SF7, 259 at SF12 |
//! | EU868 duty cycle | 1% of each hour, per sub-band |
//!
//! Sixty frames at SF7 is about forty minutes of enforced silence. Two hundred
//! and fifty-nine at SF12 is about fourteen hours. A header is one frame.
//!
//! So this is a **header relay**, exactly as `docs/architecture-vision.md` §3
//! scoped it, and `light-client` is what makes a header useful on the far end.
//!
//! ## The rule that governs all of it
//!
//! From that same section, and it is the constraint this crate is the first
//! real test of:
//!
//! > None of them may change consensus. A block is a block regardless of the
//! > medium that carried it, and the chain must never acquire a rule that
//! > depends on *how* a message arrived — that would make the transport layer
//! > consensus-critical and hand an attacker a fork by radio.
//!
//! Nothing here inspects what it carries. [`relay`] forwards opaque bundles,
//! [`fountain`] codes opaque bytes, [`frame`] wraps opaque payloads. A node
//! that could tell radio-relayed traffic from TCP traffic at the point validity
//! is decided would be a node an attacker can fork with a transmitter.
//!
//! ## ISM, not amateur
//!
//! The framing is AX.25-shaped because every packet-radio tool can decode it.
//! The *spectrum* is ISM, because amateur allocations forbid encrypted
//! transmission in most jurisdictions and `Maya2C`'s transport is ML-KEM-768
//! over Noise. Running on amateur bands would mean dropping the encryption and
//! stamping an operator's callsign on every relayed frame. See [`frame`].
//!
//! ## The three things to read first
//!
//! - [`duty`] — why the governor refuses instead of warning, and why airtime is
//!   integer arithmetic.
//! - [`fountain`] — why loss is answered by sending more rather than by asking
//!   again.
//! - [`relay`] — custody, expiry, and why a relay never looks inside.

pub mod duty;
pub mod error;
pub mod fountain;
pub mod frame;
pub mod relay;

pub use duty::{Band, DutyCycle, Settings};
pub use error::{Error, Result};
pub use fountain::{Decoder, Encoder, Symbol};
pub use frame::{Frame, Kind, NodeTag};
pub use relay::{Bundle, RelayStore};

/// The symbol payload that fits a frame at a spreading factor.
///
/// Frame capacity, minus the frame's own framing, minus the symbol's. Returns
/// `None` where the spreading factor leaves no room for data at all — which is
/// not reachable for `LoRa`'s real settings, but is the honest answer to a
/// caller that asked about a link that cannot carry this protocol.
#[must_use]
pub fn block_size_for(spreading_factor: u8) -> Option<usize> {
    frame::max_frame_for(spreading_factor)
        .checked_sub(frame::FRAME_OVERHEAD + fountain::SYMBOL_OVERHEAD)
        .filter(|size| *size > 0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn every_lora_spreading_factor_leaves_room_for_data() {
        // SF12 is the tight one: a 51-byte frame, 13 bytes of framing and 12 of
        // symbol header leaves 26 bytes of payload. Small, and not zero — a
        // protocol whose overhead ate the frame would be one nobody notices
        // until they take a radio somewhere with no coverage.
        for sf in 7..=12 {
            let size = block_size_for(sf).unwrap_or_else(|| panic!("SF{sf} carries nothing"));
            assert!(size > 0, "SF{sf}");
        }
        assert_eq!(block_size_for(12), Some(26));
        assert_eq!(block_size_for(7), Some(197));
    }

    #[test]
    fn a_header_fits_one_frame_at_every_spreading_factor_via_fountain_blocks() {
        // 144 bytes at SF12's 26-byte blocks is six symbols, which is six duty
        // cycle windows — about fifteen minutes. At SF7 it is one. Both are
        // stated rather than assumed, because the difference decides whether a
        // deployment is usable.
        const HEADER_LEN: usize = 144;
        let slow = HEADER_LEN.div_ceil(block_size_for(12).expect("SF12"));
        let fast = HEADER_LEN.div_ceil(block_size_for(7).expect("SF7"));
        assert_eq!(slow, 6);
        assert_eq!(fast, 1);
    }
}
