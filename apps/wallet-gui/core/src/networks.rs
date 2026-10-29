//! Which network a node is, decided by its genesis id and never by its name.
//!
//! A signature commits to one chain's genesis id (ADR-036), and the wallet
//! learns that id from the node it is connected to. A node's name is a claim:
//! a malicious node can call itself `maya-mainnet` with any genesis, and a
//! wallet that trusted the name would show MAINNET and sign for that chain.
//! So mainnet is a genesis id pinned here, at build time.
//!
//! [`MAINNET_GENESIS`] is empty until the genesis ceremony produces the
//! mainnet id; the ceremony runbook adds it before the release is tagged.
//! Until then no node can be shown as mainnet, which is the safe direction.

/// Genesis ids, lower-case hex, of every network this wallet calls mainnet.
pub const MAINNET_GENESIS: &[&str] = &[];

/// Names a node may use only with a pinned genesis.
const MAINNET_NAMES: &[&str] = &["maya-mainnet", "mainnet"];

/// What a node is, as far as the wallet is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkClass {
    /// A pinned mainnet genesis: coins of value.
    Mainnet,
    /// Anything not pinned and not claiming to be mainnet: coins with no value.
    Test,
    /// Claims a mainnet name with a genesis that is not pinned. The wallet
    /// refuses to sign for it.
    Impostor,
}

/// Classifies a node by the name it claims and the genesis it reports.
#[must_use]
pub fn classify(claimed_name: Option<&str>, genesis_hex: &str) -> NetworkClass {
    classify_against(claimed_name, genesis_hex, MAINNET_GENESIS)
}

fn classify_against(
    claimed_name: Option<&str>,
    genesis_hex: &str,
    pinned: &[&str],
) -> NetworkClass {
    let genesis = genesis_hex.trim().to_ascii_lowercase();
    if pinned.iter().any(|p| p.eq_ignore_ascii_case(&genesis)) {
        return NetworkClass::Mainnet;
    }
    let claims_mainnet = claimed_name
        .map(|n| n.trim().to_ascii_lowercase())
        .is_some_and(|n| MAINNET_NAMES.contains(&n.as_str()));
    if claims_mainnet {
        NetworkClass::Impostor
    } else {
        NetworkClass::Test
    }
}

/// The first and last eight hex characters of a genesis id: what a person
/// compares against the network's published fingerprint before signing.
#[must_use]
pub fn fingerprint(genesis_hex: &str) -> String {
    let g = genesis_hex.trim();
    if g.len() <= 16 {
        return g.to_string();
    }
    format!("{}\u{2026}{}", &g[..8], &g[g.len() - 8..])
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: &str = "aa11bb22cc33dd44ee55ff6600778899aa11bb22cc33dd44ee55ff6600778899";
    const OTHER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_mainnet_name_without_the_pinned_genesis_is_an_impostor() {
        assert_eq!(
            classify_against(Some("maya-mainnet"), OTHER, &[PINNED]),
            NetworkClass::Impostor
        );
        assert_eq!(
            classify_against(Some(" MainNet "), OTHER, &[PINNED]),
            NetworkClass::Impostor
        );
    }

    #[test]
    fn the_pinned_genesis_is_mainnet_whatever_the_node_calls_it() {
        assert_eq!(
            classify_against(Some("maya-mainnet"), PINNED, &[PINNED]),
            NetworkClass::Mainnet
        );
        assert_eq!(
            classify_against(None, &PINNED.to_uppercase(), &[PINNED]),
            NetworkClass::Mainnet
        );
    }

    #[test]
    fn anything_else_is_a_test_network() {
        assert_eq!(
            classify_against(Some("maya-testnet-1"), OTHER, &[PINNED]),
            NetworkClass::Test
        );
        assert_eq!(classify_against(None, OTHER, &[PINNED]), NetworkClass::Test);
        assert_eq!(
            classify_against(Some("mainnet-fork"), OTHER, &[PINNED]),
            NetworkClass::Test
        );
    }

    #[test]
    fn before_the_ceremony_nothing_is_mainnet() {
        assert!(MAINNET_GENESIS.is_empty() || MAINNET_GENESIS.iter().all(|g| g.len() == 64));
        if MAINNET_GENESIS.is_empty() {
            assert_eq!(
                classify(Some("maya-mainnet"), OTHER),
                NetworkClass::Impostor
            );
        }
    }

    #[test]
    fn a_fingerprint_shows_both_ends() {
        assert_eq!(fingerprint(PINNED), "aa11bb22\u{2026}00778899");
        assert_eq!(fingerprint("abcd"), "abcd");
    }
}
