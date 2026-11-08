//! `maya-sentinel`: a read-only watchdog for a running `Maya2C` network
//! (ADR-077, the live layer of the watchdog directive).
//!
//! It polls each node's own RPC, runs the detectors in [`detect`], turns
//! conditions into one alert per incident ([`incident`]) and pushes them to a
//! phone through ntfy ([`notify`]). It never signs, never submits, and never
//! acts on the chain: an alert is a message to a human, and the response is a
//! runbook step (`docs/runbooks/sentinel.md`).

#![warn(missing_docs)]

pub mod config;
pub mod detect;
pub mod incident;
pub mod notify;
pub mod obs;
pub mod rpc;
