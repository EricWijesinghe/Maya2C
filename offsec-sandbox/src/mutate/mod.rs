//! Structure-aware mutators for the three surfaces the brief names.
//!
//! Each surface exposes `mutate(seed, rng) -> Vec<u8>` and `seeds() ->
//! Vec<Vec<u8>>`. A mutator first tries to decode the seed into its typed form;
//! if that succeeds it mutates a *field* and re-encodes, which is what reaches
//! logic past `from_bytes`. If the seed does not decode — an early corpus is
//! mostly junk — it falls back to the byte-level mutations in [`bytes`], so the
//! decoder itself still gets hammered.

pub mod bytes;
pub mod handshake;
pub mod tx;
pub mod wasm;

use crate::{Rng, Surface};

/// Mutates one input for `surface`.
#[must_use]
pub fn mutate(surface: Surface, seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    match surface {
        Surface::Transaction | Surface::Block => tx::mutate(seed, rng),
        Surface::Wasm => wasm::mutate(seed, rng),
        Surface::Handshake => handshake::mutate(seed, rng),
    }
}

/// The seed corpus for `surface`: valid, encoded values to mutate from.
#[must_use]
pub fn seeds(surface: Surface) -> Vec<Vec<u8>> {
    match surface {
        Surface::Transaction | Surface::Block => tx::seeds(),
        Surface::Wasm => wasm::seeds(),
        Surface::Handshake => handshake::seeds(),
    }
}
