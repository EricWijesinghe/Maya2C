//! Maya2C Ledger application — protocol layer.
//!
//! # This is a feasibility spike, not a product
//!
//! The question it answers is whether a Ledger can sign a Maya2C transaction at
//! all. The answer is not obvious and needs measurement:
//!
//! A Maya2C signature is a **hybrid pair** — ML-DSA-65 (FIPS 204) and
//! SLH-DSA-SHA2-128s (FIPS 205), 11,165 bytes together — and
//! `HybridVerifyingKey::verify` checks **both**. A device that produces only the
//! lattice half has produced nothing the chain will accept.
//!
//! The lattice half is plausible: `fips204` is `#![no_std]`, allocates no heap,
//! and ships an embedded example. The hash-based half is the open question, and
//! the evidence is not encouraging — `slh-dsa`'s own documentation warns it
//! "allocates signatures and intermediate values on the stack, which may cause
//! problems for environments with limited stack space", and its key generation
//! overflowed a **1 MB** stack on a desktop during this project's genesis
//! ceremony work. A Ledger's app RAM is measured in kilobytes.
//!
//! So: build it, instrument it, and report numbers. If the hash-based half does
//! not fit, that is the finding, and it is worth more than an app that signs
//! half a signature.
//!
//! # Why the protocol layer is a library
//!
//! Everything here is `no_std` but host-testable, and holds no key material.
//! Chunk assembly, sequence checking and path validation are where the bugs
//! are, and none of them need a device — or an emulator — to exercise. The
//! device binary in `main.rs` is a thin shell over this.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod apdu;
pub mod derive;
pub mod sign;
