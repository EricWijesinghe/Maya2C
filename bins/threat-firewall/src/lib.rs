//! Per-host firewall worker for the threat-intel registry.
//!
//! Polls one co-located node for two things and joins them:
//!
//! 1. **Indicators** from chain state (`threat_indicators`) — keyed by author,
//!    carrying no address;
//! 2. **Addresses** from the node's own connections (`threat_peer_addresses`) —
//!    this node's knowledge of where each author connected from.
//!
//! [`policy::decide`] turns those into IPs to block and unblock, a pure
//! function tested without a node; [`worker::Worker`] applies the difference
//! through a [`sink::FirewallSink`].
//!
//! ## One worker per host, pushing nothing anywhere
//!
//! There is no controller that writes rules onto other machines. Every host
//! that runs a node runs its own worker against its own node, so what spreads
//! across the network is the *indicator*, by consensus, and each host enforces
//! it from what it saw itself. A central rule pusher would be a
//! lateral-movement control plane; this has nothing to take over.
//!
//! ## Why not XDP
//!
//! The XDP program in `ebpf-net` judges only relay datagrams and passes all
//! other traffic, and it belongs to the node's relay: a second loader would
//! detach it. General ingress filtering is netfilter's job, so the sinks are
//! nftables and iptables. The node already mirrors its own quarantines into the
//! XDP blocklist for relay traffic.
//!
//! ## What a block costs an honest host
//!
//! Peers can share an address — NAT, a cloud egress IP. Blocking an offender's
//! address blocks everyone behind it. That is the price of acting at the IP
//! layer at all, and why loopback and unspecified addresses are never blocked.

pub mod error;
pub mod policy;
pub mod rpc;
pub mod sink;
pub mod source;
pub mod worker;

pub use error::{FirewallError, Result};
pub use policy::{Plan, decide};
pub use sink::{Backend, CommandSink, DryRunSink, FirewallSink, Invocation, SinkAction};
pub use source::{Observation, ThreatSource};
pub use worker::{StepReport, Worker};
