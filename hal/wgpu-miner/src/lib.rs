//! Cross-platform GPU mining for `Maya2C`'s hashimoto proof of work.
//!
//! # Why hashimoto and not `ArgonBlake`
//!
//! `cuda-miner` accelerates `ArgonBlake`, the chain's proof of work before
//! `DAG_ACTIVATION_HEIGHT`. This crate does not duplicate it, for a reason that
//! is about WGSL rather than about effort.
//!
//! **WGSL has no native 64-bit integer type.** Argon2id and `BLAKE2b` are u64
//! algorithms throughout. Emulating u64 as pairs of u32 through `BLAKE2b`'s G
//! function is possible, slow, and a correctness surface on a consensus hash —
//! where one wrong bit produces blocks the network rejects silently. CUDA has
//! native u64, which is why that port belongs there and this one does not.
//!
//! Hashimoto is the opposite case. Its mix is `[u32; 32]`, an item is
//! `[u32; 16]`, and the only arithmetic is FNV: a 32-bit multiply and an xor.
//! It is also memory-bandwidth-bound by design — `crypto::dag::hashimoto`'s own
//! documentation calls the arithmetic "deliberately trivial, so that the DRAM
//! round trip is the entire cost" — which is exactly the shape a GPU helps
//! with.
//!
//! # What runs where
//!
//! ```text
//! host   seed = BLAKE3-512(mix_key, header_hash ‖ nonce_le)
//! GPU    mix  = 64 data-dependent FNV accumulations over the dataset
//! host   result = BLAKE3(mix_key, seed ‖ compress(mix))
//! ```
//!
//! Both BLAKE3 calls are the node's own, reached through
//! `crypto::dag::hashimoto::{seed_for, finish}`. No hash is reimplemented here
//! — only the loop between them, in `shaders/hashimoto.wgsl`.
//!
//! # The `gpu` feature is off by default
//!
//! Same rule as `cuda-miner`'s `cuda` feature, and for the same reason:
//! `cargo build --workspace` has to stay green on a runner with no adapter and
//! no graphics driver. With no feature selected this crate is the host pipeline
//! and the CPU reference, which is what the parity tests compare against
//! anyway.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;
pub mod reference;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use error::MinerError;
