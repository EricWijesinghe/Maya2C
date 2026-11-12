//! `maya-invariant-monitor`: what anybody can check about a running `Maya2C`
//! network from its public RPC, without trusting the foundation's watcher.
//!
//! The sentinel's detectors (halt, fork, supply adds up, supply split, supply
//! fell) run unchanged over the same polls. This crate adds the checks that
//! need the public consensus data the gateway serves (`get_checkpoint`):
//! heights never go backwards, finality never reverts, and a finalized block
//! is never over the byte cap (invariant 33). It never signs or submits.

#![warn(missing_docs)]

pub mod checks;
pub mod public;
