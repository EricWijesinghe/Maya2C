//! Which node methods the gateway will call, and — more importantly — which it
//! will not.
//!
//! # Why this exists
//!
//! The node's JSON-RPC server has **no authentication**. That is a reasonable
//! design for an interface that assumes a trusted network position: it is bound
//! inside a pod network, and the deployment is what keeps it private.
//!
//! A gateway breaks that assumption by construction. It terminates public HTTP
//! and forwards to the node, so every method reachable through it is a method
//! reachable by anyone who can reach the gateway. Inheriting the node's
//! "everything is allowed" posture would publish the miner interface to the
//! internet.
//!
//! # Default deny
//!
//! [`is_allowed`] returns `false` for anything not named here. A method added
//! to the node does not become reachable through the gateway until somebody
//! adds it to this list and says why — which is the review step that catches
//! the next `submit_block`.
//!
//! # What is deliberately excluded
//!
//! - **`get_mining_candidate`** — hands out the header a miner is searching. A
//!   public endpoint for it is free work for anyone building a competing
//!   template, and it reveals the node's mempool selection.
//! - **`submit_block`** — accepts a block for validation and propagation.
//!   Exposing it publicly means anyone can make every node on the network do
//!   full block validation on demand, which is an amplification vector with no
//!   upside: a real miner talks to its own node.
//!
//! - **`get_headers`**, **`get_snapshot_manifest`** and
//!   **`get_snapshot_chunk`** — what a pruned node bootstraps from. A snapshot
//!   chunk is a slice of the whole committed state and a header range is up to
//!   2,000 headers a call, so both are cheap to ask for and expensive to
//!   answer. Node-to-node work, on the node's own port, not a public endpoint.
//! - **`get_tip_height`** — harmless in itself, and excluded only to keep the
//!   public surface to what the REST layer already covers.
//!
//! Both stay reachable on the node's own RPC port, to the operator who is
//! already inside the trust boundary.

/// Node RPC methods the gateway is permitted to call.
///
/// Read-only, plus the one write that is meant to be public: submitting a
/// signed transaction is what a public chain is for.
pub const ALLOWED_METHODS: &[&str] = &[
    "get_balance",
    "get_block_by_height",
    "get_supply",
    "send_raw_transaction",
];

/// Methods that exist on the node and must never be proxied.
///
/// Not merely absent from [`ALLOWED_METHODS`] — named, so the exclusion is a
/// decision with a test behind it rather than an oversight that looks the same.
pub const DENIED_METHODS: &[&str] = &[
    "get_mining_candidate",
    "submit_block",
    "get_headers",
    "get_snapshot_manifest",
    "get_snapshot_chunk",
    "get_tip_height",
];

/// Whether the gateway may forward `method` to the node.
#[must_use]
pub fn is_allowed(method: &str) -> bool {
    ALLOWED_METHODS.contains(&method)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_denied_methods_are_refused() {
        // The assertion this module exists for. If any of these ever returned
        // true, the gateway would be publishing the miner interface, or
        // serving whole-state snapshot work to anyone who asks.
        for method in DENIED_METHODS {
            assert!(!is_allowed(method), "{method} must not be proxied");
        }
    }

    #[test]
    fn an_unknown_method_is_refused() {
        // Default deny: a method added to the node is not reachable here until
        // somebody adds it deliberately.
        assert!(!is_allowed("get_state_root"));
        assert!(!is_allowed(""));
        assert!(!is_allowed("send_raw_transaction "));
    }

    #[test]
    fn the_allowed_methods_are_allowed() {
        for method in ALLOWED_METHODS {
            assert!(is_allowed(method), "{method} should be reachable");
        }
    }

    #[test]
    fn the_two_lists_do_not_overlap() {
        // A method in both lists would mean the deny list was decorative.
        for denied in DENIED_METHODS {
            assert!(
                !ALLOWED_METHODS.contains(denied),
                "{denied} is in both lists"
            );
        }
    }
}
