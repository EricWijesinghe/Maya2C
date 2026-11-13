//! What `float-check` reads: the code the node binary links, derived from the
//! manifests rather than kept by hand, so a crate the node starts to link is
//! scanned unless an [`EXCLUDE`] entry says why not.
//!
//! Dev-dependencies are not followed (they are not linked). Normal, build and
//! `[target.'cfg(..)']` dependencies are, and `workspace = true` entries are
//! resolved against the root `[workspace.dependencies]`, renames included.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};

/// The binary whose link graph is the scope.
pub const NODE_BIN: &str = "bins/maya2c-node";

/// Linked code that is not scanned: `(repo-relative dir or file, why no
/// consensus rule depends on it)`. An entry needs a reviewer.
pub const EXCLUDE: &[(&str, &str)] = &[
    (
        "crates/node/src/rpc",
        "JSON-RPC server: per-IP rate limiting (`limit.rs`) and response formatting; a node answers queries with it and decides nothing a block contains",
    ),
    (
        "crates/node/src/network",
        "libp2p transport: gossipsub peer scores and backoff are local to each node and never enter a block or the state root",
    ),
    (
        "crates/node/src/metrics",
        "Prometheus gauges and histograms, read by operators only",
    ),
    (
        "crates/archive",
        "copies finalized blocks to disk or IPFS after the fact; progress and timing figures only",
    ),
    (
        "crates/iso20022",
        "translates payments into ISO 20022 messages for off-chain systems; it writes no state",
    ),
    (
        "hal/ebpf-net",
        "XDP packet filter in front of the p2p port; it drops packets and validates nothing",
    ),
];

/// Consensus code that must stay in scope. `float-check` fails if any of
/// these contributes no scanned file, so the scope cannot shrink silently.
pub const MUST_SCAN: &[&str] = &[
    "crates/node/src/state",
    "crates/node/src/consensus",
    "crates/node/src/core",
    "crates/node/src/crypto",
    "crates/node/src/oracle",
    "crates/node/src/governance",
    "crates/node/src/neural_gas",
    "crates/node/src/upgrade.rs",
    "crates/node/src/genesis.rs",
    "crates/ledger-math",
    "crates/dex",
    "crates/governance",
    "crates/fee-market",
    "crates/vrf",
    "crates/crypto-pq",
    "crates/dag-bft",
    "crates/staking",
    "crates/vm",
    "crates/lattice-pow",
    "crates/amount",
    "crates/stateless-core",
    "crates/htlc-lattice",
    "crates/identity",
    "crates/rwa",
    "crates/threat-intel",
    "hal/iot-anchor",
    "bins/maya2c-node/src/bft.rs",
];

/// The node's link graph: workspace crate dirs and every package name in it.
pub struct Linked {
    /// Repo-relative dirs of the path crates, the node binary included.
    pub dirs: Vec<String>,
    /// Path crates, their registry dependencies, and those dependencies'
    /// closure in `Cargo.lock` (all versions of a name are merged).
    pub names: BTreeSet<String>,
}

/// `path` is `entry` itself or lies in the dir `entry`.
pub fn under(path: &str, entry: &str) -> bool {
    path == entry || path.strip_prefix(entry).is_some_and(|r| r.starts_with('/'))
}

pub fn linked(root: &Path) -> Result<Linked, String> {
    let ws = workspace_deps(root)?;
    let mut dirs = Vec::new();
    let mut names = BTreeSet::new();
    let mut registry = BTreeSet::new();
    let mut queue = VecDeque::from([root.join(NODE_BIN)]);
    while let Some(dir) = queue.pop_front() {
        let rel = rel_dir(root, &dir);
        if dirs.contains(&rel) {
            continue;
        }
        let manifest = dir.join("Cargo.toml");
        let text =
            std::fs::read_to_string(&manifest).map_err(|e| format!("{rel}/Cargo.toml: {e}"))?;
        let doc: toml::Table = text.parse().map_err(|e| format!("{rel}/Cargo.toml: {e}"))?;
        if let Some(name) = doc
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
        {
            names.insert(name.to_owned());
        }
        for table in dep_tables(&doc) {
            for (key, value) in table {
                let (name, path) = resolve(key, value, &ws);
                match path {
                    Some(Base::Crate(p)) => queue.push_back(normalize(&dir.join(p))),
                    Some(Base::Root(p)) => queue.push_back(normalize(&root.join(p))),
                    None => {
                        registry.insert(name);
                    }
                }
            }
        }
        dirs.push(rel);
    }
    let lock =
        std::fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    names.extend(lock_closure(&lock, &registry)?);
    Ok(Linked { dirs, names })
}

/// Where a path dependency's `path` is relative to.
#[derive(Debug, PartialEq)]
pub enum Base {
    Crate(String),
    Root(String),
}

/// `(package name, path)` of one dependency entry; `workspace = true` reads
/// the root entry, and a `package =` rename on either side wins over the key.
pub fn resolve(key: &str, value: &toml::Value, ws: &toml::Table) -> (String, Option<Base>) {
    let base = value
        .get("workspace")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
        .then(|| ws.get(key))
        .flatten();
    let field = |k: &str| {
        let own = value.get(k).and_then(toml::Value::as_str);
        own.or_else(|| base.and_then(|b| b.get(k)).and_then(toml::Value::as_str))
    };
    let name = field("package").unwrap_or(key).to_owned();
    let path = match (value.get("path").and_then(toml::Value::as_str), base) {
        (Some(p), _) => Some(Base::Crate(p.to_owned())),
        (None, Some(b)) => b
            .get("path")
            .and_then(toml::Value::as_str)
            .map(|p| Base::Root(p.to_owned())),
        (None, None) => None,
    };
    (name, path)
}

/// `[dependencies]`, `[build-dependencies]` and both under every `[target.*]`.
pub fn dep_tables(doc: &toml::Table) -> Vec<&toml::Table> {
    const KINDS: [&str; 2] = ["dependencies", "build-dependencies"];
    let targets = doc.get("target").and_then(toml::Value::as_table);
    let scopes = std::iter::once(doc).chain(
        targets
            .into_iter()
            .flat_map(|t| t.values().filter_map(toml::Value::as_table)),
    );
    scopes
        .flat_map(|s| {
            KINDS
                .iter()
                .filter_map(|k| s.get(*k).and_then(toml::Value::as_table))
        })
        .collect()
}

/// Every package reachable in `Cargo.lock` from `roots`, the roots included.
/// Registry entries in a lock list no dev-dependencies, so this is the linked
/// closure; merging versions of one name can only over-include.
pub fn lock_closure(lock: &str, roots: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let doc: toml::Table = lock.parse().map_err(|e| format!("Cargo.lock: {e}"))?;
    let packages = doc
        .get("package")
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let deps_of = |name: &str| -> Vec<String> {
        packages
            .iter()
            .filter(|p| p.get("name").and_then(toml::Value::as_str) == Some(name))
            .filter_map(|p| p.get("dependencies").and_then(toml::Value::as_array))
            .flatten()
            .filter_map(toml::Value::as_str)
            .filter_map(|d| d.split(' ').next())
            .map(str::to_owned)
            .collect()
    };
    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();
    while let Some(name) = queue.pop_front() {
        if seen.insert(name.clone()) {
            queue.extend(deps_of(&name));
        }
    }
    Ok(seen)
}

fn workspace_deps(root: &Path) -> Result<toml::Table, String> {
    let text =
        std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let doc: toml::Table = text.parse().map_err(|e| format!("Cargo.toml: {e}"))?;
    Ok(doc
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default())
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn rel_dir(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(normalize(root))
        .or_else(|_| dir.strip_prefix(root))
        .unwrap_or(dir)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_rename_and_its_path_are_resolved_from_the_root() {
        let ws: toml::Table =
            "nn = { package = \"candle-core\", version = \"0.8\" }\nm = { path = \"crates/m\" }\n"
                .parse()
                .unwrap();
        let member: toml::Table =
            "nn = { workspace = true }\nm = { workspace = true }\nlocal = { path = \"../l\" }\nserde = \"1\"\n"
                .parse()
                .unwrap();
        let got: Vec<_> = member.iter().map(|(k, v)| resolve(k, v, &ws)).collect();
        assert_eq!(
            got,
            [
                ("local".to_owned(), Some(Base::Crate("../l".to_owned()))),
                ("m".to_owned(), Some(Base::Root("crates/m".to_owned()))),
                ("candle-core".to_owned(), None),
                ("serde".to_owned(), None),
            ]
        );
    }

    #[test]
    fn build_and_target_specific_tables_are_read_and_dev_dependencies_are_not() {
        let doc: toml::Table = "[dependencies]\na = \"1\"\n[build-dependencies]\nb = \"1\"\n[dev-dependencies]\nd = \"1\"\n[target.'cfg(windows)'.dependencies]\nt = \"1\"\n"
            .parse()
            .unwrap();
        let keys: Vec<&str> = dep_tables(&doc)
            .iter()
            .flat_map(|t| t.keys())
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["a", "b", "t"]);
    }

    #[test]
    fn an_ml_runtime_two_hops_away_is_in_the_lock_closure() {
        let lock = r#"
            [[package]]
            name = "a"
            dependencies = ["b 1.0.0", "serde"]
            [[package]]
            name = "b"
            version = "1.0.0"
            dependencies = ["ort"]
            [[package]]
            name = "ort"
            [[package]]
            name = "unrelated"
            dependencies = ["tch"]
        "#;
        let got = lock_closure(lock, &BTreeSet::from(["a".to_owned()])).unwrap();
        assert_eq!(
            got,
            BTreeSet::from(["a", "b", "ort", "serde"].map(str::to_owned))
        );
    }

    #[test]
    fn under_matches_a_dir_and_not_a_sibling_with_the_same_prefix() {
        assert!(under("crates/vm/src/a.rs", "crates/vm"));
        assert!(under("crates/vm", "crates/vm"));
        assert!(!under("crates/vmx/src/a.rs", "crates/vm"));
    }
}
