//! Reference applications (Master Prompt 30 §1).
//!
//! Five apps, each a template a team can copy, built only from libraries the
//! chain already has:
//!
//! | App | Module | Built on |
//! |---|---|---|
//! | Point-of-sale payments | [`payments`] | smart accounts (session keys, limits), clear signing |
//! | DEX front end | [`dex_front`] | the native AMM, look-alike token guard, clear-signing review |
//! | DAO | [`dao`] | governance proposals and tally, treasury stages and epoch limit |
//! | Quantum-safe vault | [`vault`] | vault delays, guardian recovery, PQ viewing keys, exposure check |
//! | NFT game | `contracts/nft-game` | the WASM VM — **blocked**, see below |
//!
//! **Where they run.** In-process, against the same crates the node links. Not
//! on a testnet: none is public, and smart accounts are not yet a node
//! transaction type (`docs/architecture-vision.md`). A template that pretended
//! to talk to a network would teach the wrong thing.
//!
//! **The NFT game is blocked by a VM gap, not by this crate.** The VM gives a
//! contract no way to learn who called it (there is no `caller` host function;
//! `crates/node/src/state/vm_exec.rs` knows the caller and does not pass it).
//! A contract that records ownership therefore cannot check it. The contract is
//! written, and its test demonstrates the consequence — see
//! `docs/adr/ADR-026-contract-caller-identity.md`.
//!
//! **No stablecoin exists on the chain**, so the payments app moves the native
//! coin in base units. Its structure is unchanged by the token it moves.

pub mod dao;
pub mod dex_front;
pub mod payments;
pub mod vault;
pub mod wallet;
