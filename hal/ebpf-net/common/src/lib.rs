//! What the XDP program and user space must agree on, byte for byte.
//!
//! The relay path has two halves that are compiled by different toolchains for
//! different machines: an XDP program for `bpfel-unknown-none`, checked by the
//! kernel's verifier, and the node's user-space receiver. Anything both halves
//! decide — where a field sits, when a datagram is malformed, when a source has
//! sent too much — lives here once, so the host's test suite exercises the same
//! function the kernel runs.
//!
//! - [`header`]: the 56-byte relay header and the exact datagram length it
//!   implies.
//! - [`rate`]: a per-source token bucket in integer arithmetic.
//! - [`verdict`]: the order in which the kernel refuses a relay datagram.
//! - [`packet`]: Ethernet, IPv4/IPv6 and UDP, for user space and for tests.
//! - [`maps`]: the records stored in the kernel's maps, and their names.
//!
//! # Why the kernel's functions are `#[inline(always)]`
//!
//! Everything the XDP program calls from here is forced inline, and that is a
//! verifier requirement found against a real kernel, not a tuning choice. A
//! BPF-to-BPF call gets a fresh frame with only five argument registers. Left
//! as calls, LLVM spilled a sixth argument through R11 ("R11 is invalid"), and
//! read a callee-saved register it considered undefined while building a
//! returned `Result` ("R9 !`read_ok`"). Inlined, the program has no such calls.
//!
//! # What is deliberately absent
//!
//! No cryptography. The verifier cannot run an AEAD over a 1,200-byte payload
//! inside its instruction budget, and a check the kernel cannot finish is not a
//! check. The kernel refuses what is *structurally* wrong or *too much*; whether
//! a datagram came from a peer holding the key is decided in user space. See
//! `docs/ebpf-net.md`.

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]

pub mod header;
pub mod maps;
pub mod packet;
pub mod rate;
pub mod verdict;

pub use header::{HeaderError, RelayHeader};
pub use maps::{BlockEntry, Config, Counter};
pub use rate::{Bucket, RateLimit};
pub use verdict::{Judgement, Verdict, judge};
