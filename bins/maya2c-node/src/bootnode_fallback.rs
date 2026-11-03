//! Built-in bootnodes, dialed only when no peer answers.
//!
//! Configured bootnodes (`--bootnode`, `[network] bootnodes`) are the normal
//! way in. This list is the last resort for a node whose operator has none
//! reachable: it is dialed after `FALLBACK_AFTER_ROUNDS` redial rounds with
//! zero connected peers, and never while any peer is connected.

use std::error::Error;

use libp2p::Multiaddr;
use libp2p::multiaddr::Protocol;

/// Addresses dialed when this node has had no peer for a while.
///
/// Empty on purpose. An entry names a live host for every node that ever
/// starts, so a stale or wrong one dials a stranger on each start. Operators
/// list their own bootnodes at several providers in `--bootnode` or
/// `[network] bootnodes`; see docs/runbooks/validator-start-guards.md. Each entry
/// must end in `/p2p/<peer id>`, which pins the peer.
pub(crate) const FALLBACK_BOOTNODES: &[&str] = &[];

/// Consecutive redial rounds with no connected peer before the fallback
/// list is dialed. Rounds are `REDIAL_INTERVAL` apart (10 s), so 3 is 30 s:
/// long enough that a slow start does not pull in the fallback.
pub(crate) const FALLBACK_AFTER_ROUNDS: u32 = 3;

/// Whether the fallback list should be dialed this round.
pub(crate) fn fallback_due(connected_peers: usize, empty_rounds: u32) -> bool {
    connected_peers == 0 && empty_rounds >= FALLBACK_AFTER_ROUNDS
}

/// Parses the shipped fallback list. Fails at start-up, not mid-run.
pub(crate) fn fallback_addrs() -> Result<Vec<Multiaddr>, Box<dyn Error>> {
    parse_list(FALLBACK_BOOTNODES)
}

fn parse_list(entries: &[&str]) -> Result<Vec<Multiaddr>, Box<dyn Error>> {
    entries
        .iter()
        .map(|entry| {
            let addr: Multiaddr = entry
                .parse()
                .map_err(|e| format!("fallback bootnode {entry}: {e}"))?;
            if addr.iter().any(|p| matches!(p, Protocol::P2p(_))) {
                Ok(addr)
            } else {
                Err(format!("fallback bootnode {entry} has no /p2p/<peer id> suffix").into())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str =
        "/dns4/seed.example/tcp/443/wss/p2p/12D3KooWJqH5AUQ1zPkMke18rcc8VHN2TWQ5XudGX3tP8zPLuyjB";

    #[test]
    fn fallback_waits_for_the_threshold() {
        assert!(!fallback_due(0, FALLBACK_AFTER_ROUNDS - 1));
        assert!(fallback_due(0, FALLBACK_AFTER_ROUNDS));
    }

    #[test]
    fn a_connected_peer_keeps_the_fallback_unused() {
        assert!(!fallback_due(1, u32::MAX));
    }

    #[test]
    fn an_entry_with_a_peer_id_parses() {
        assert_eq!(parse_list(&[PEER]).map(|v| v.len()).ok(), Some(1));
    }

    #[test]
    fn an_entry_without_a_peer_id_is_refused() {
        assert!(parse_list(&["/dns4/seed.example/tcp/443/wss"]).is_err());
    }

    #[test]
    fn a_malformed_entry_is_refused() {
        assert!(parse_list(&["not-a-multiaddr"]).is_err());
    }

    #[test]
    fn the_shipped_list_parses() {
        assert!(fallback_addrs().is_ok());
    }
}
