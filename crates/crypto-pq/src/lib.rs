//! Post-quantum primitives, isolated behind concrete APIs.
//!
//! Two modules, one for each thing the chain needs:
//!
//! - [`sig`] — SLH-DSA-SHA2-128s (FIPS 205), the hash-based half of every
//!   transaction signature. Its lattice partner, ML-DSA-65, lives in
//!   `custom_l1_node::crypto::keys`.
//! - [`kem`] — ML-KEM-768 (FIPS 203), the key encapsulation the P2P transport
//!   layers over its Noise session.
//! - [`suite`] — the signature-suite registry (ADR-007): Ed25519, ML-DSA-65/87,
//!   SLH-DSA-SHA2-128s / SHAKE-256f, and the ML-DSA + SLH-DSA hybrid, each
//!   behind one trait and a one-byte id.
//! - [`envelope`] — the suite-tagged signature encoding.
//! - [`kem_suite`] — ML-KEM-768/1024, HQC-128/256 (draft), X-Wing, and the
//!   ML-KEM + HQC dual-KEM combiners (ADR-009).
//! - [`agility`] — default-suite policy, the security audit, and migration
//!   off a deprecated suite.
//!
//! ## Why this crate exists at all
//!
//! Both backends are generic over their parameter set, so their hot code
//! monomorphizes into whichever crate *instantiates* it. A
//! `[profile.dev.package.slh-dsa]` or `[profile.dev.package.ml-kem]` override —
//! the trick that keeps `fips204` usable in dev builds — therefore does
//! nothing: it optimizes a crate that contains none of the work. Both were
//! measured:
//!
//! | | dependency override only | instantiating crate optimized |
//! |---|---|---|
//! | SLH-DSA-SHA2-128s sign | 4468 ms | 140 ms |
//! | ML-KEM-768 handshake | 5805 µs | 785 µs |
//!
//! So every instantiation is confined here, behind APIs that name concrete byte
//! arrays rather than type parameters, and the root `Cargo.toml` optimizes
//! *this* crate in dev. That keeps the node's own code unoptimized and
//! debuggable, which is the property the root profile overrides exist to
//! preserve — raising `[profile.dev]` workspace-wide would sacrifice it.
//!
//! Keeping the generics in one place has a second benefit: a caller cannot
//! reach for the wrong parameter set by accident, because no other parameter
//! set is nameable from outside.

pub mod agility;
pub mod envelope;
pub mod hqc;
pub mod kem;
pub mod kem_suite;
pub mod sig;
pub mod suite;
