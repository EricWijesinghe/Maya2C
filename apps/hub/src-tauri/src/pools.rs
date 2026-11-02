//! The nodes a rig may ask for work (ADR-073 decision 5).
//!
//! The miner trusts the work it is given (`share.rs`), so a node the Hub does
//! not know cannot be offered until the miner recomputes the share header
//! itself (ADR-073 Phase 2). The list is compiled in for now; the signed
//! release list (#258) replaces it later. The Hub accepts a pool by id only,
//! never by a URL typed in the mining path.

use serde::Serialize;

/// One endpoint a rig may mine through.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Pool {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
}

/// The public testnet node, the default and currently the only choice.
pub const POOLS: &[Pool] = &[Pool {
    id: "public",
    name: "Maya2C public node",
    url: "https://rpc.maya2c.dev/rpc",
}];

/// The URL behind a pool id. An unknown id is refused, not defaulted.
///
/// # Errors
///
/// The id is not on the list.
pub fn resolve(id: &str) -> Result<&'static str, String> {
    POOLS
        .iter()
        .find(|pool| pool.id == id)
        .map(|pool| pool.url)
        .ok_or_else(|| format!("unknown pool '{id}'; the Hub only mines through its listed nodes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_pool_resolves_and_an_unknown_one_is_refused() {
        assert_eq!(resolve("public"), Ok("https://rpc.maya2c.dev/rpc"));
        let err = resolve("http://evil.example").unwrap_err();
        assert!(err.contains("unknown pool"), "{err}");
    }
}
