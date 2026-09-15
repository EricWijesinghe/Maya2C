//! Block relay over UDP, with an optional XDP/AF_XDP receive path.
//!
//! Gossip carries every block over the libp2p stack: TCP, Noise, the ML-KEM
//! layer, yamux, gossipsub. Every byte of that is ciphertext to the kernel, so
//! an XDP program cannot look inside it. This crate adds a second, narrower
//! route for **blocks only**, whose datagrams carry a fixed header the kernel
//! *can* read:
//!
//! ```text
//! sender                                     receiver
//! Block::to_bytes ─► BlockSealer ─► UDP ─►  NIC ─► XDP (maya_relay)
//!                    1160-byte chunks          │  blocklist → rate → header
//!                    XChaCha20-Poly1305        ├─ drop at the driver
//!                    under a per-peer key      └─ redirect ─► AF_XDP socket
//!                                                             │
//!                              RelayReceiver: open ─► reassemble ─► node
//! ```
//!
//! # What the kernel decides, and what it cannot
//!
//! The XDP program ([`maya_ebpf_net_common::verdict`]) drops relay datagrams
//! from blocked sources, over their rate, or structurally wrong, before the
//! kernel allocates anything for them. It cannot check a key: the AEAD is
//! decided in user space, and a datagram that fails it is dropped *without*
//! blaming anyone, because the source address of a UDP datagram is whatever its
//! sender wrote. Only the peer guard's quarantines, which come from
//! authenticated connections, reach the kernel's blocklist.
//!
//! # A relay decides nothing
//!
//! A reassembled body goes to the node's gossip block path unchanged. There is
//! no rule anywhere that depends on how a block arrived
//! (`docs/architecture-vision.md` §3).
//!
//! # Modules
//!
//! - [`seal`]: relay keys and per-chunk sealing.
//! - [`chunker`]: a block into datagrams.
//! - [`reassembly`]: datagrams back into a block, within fixed memory.
//! - [`receiver`]: the key book and the per-datagram pipeline.
//! - [`ingress`]: starting the kernel path (Linux, `xdp` feature).

// Denied everywhere but `linux`, so "`unsafe` lives only in the kernel path" is
// something the compiler checks rather than a review convention.
#![deny(unsafe_code)]

pub mod chunker;
pub mod error;
pub mod ingress;
pub mod reassembly;
pub mod receiver;
pub mod seal;

#[cfg(all(target_os = "linux", feature = "xdp"))]
#[allow(unsafe_code)]
pub mod linux;

pub use chunker::BlockSealer;
pub use error::RelayError;
pub use ingress::{AttachMode, XdpIngress, XdpIngressConfig, ZeroCopy};
pub use maya_ebpf_net_common as common;
pub use reassembly::{Absorbed, Limits, Misbehaviour, Reassembler, Refused};
pub use receiver::{
    Dropped, KeyBook, ReceiverStats, RelayEvent, RelayReceiver, SharedKeyBook, SharedReassembler,
};
pub use seal::{CONTRIBUTION_LEN, DirectionalKeys, RelayKey, Role, derive_keys};
