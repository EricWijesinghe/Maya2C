//! `config.toml`: the operational settings a node reads from disk.
//!
//! # What is deliberately absent, and why it is absent from the *type*
//!
//! There is no consensus field here. Not "there is one and you should not set
//! it" — there is no field, so no configuration file can express one.
//!
//! [`crate::consensus::ChainConfig`] carries three values and every one of them
//! is consensus:
//!
//! | Field | What a per-host override does |
//! |---|---|
//! | `verify_pow` | `false` makes a node that accepts **any** block |
//! | `pow_limit` | a different difficulty floor is a different network |
//! | `dag.activation_height` | decides which proof-of-work rule applies when |
//!
//! A node whose file disagrees with its peers forks — silently, and usually
//! only under load. Chain rules come from `genesis.json`, which is one shared
//! artefact every operator diffs against the ceremony's `COMMITMENT.txt`. That
//! is the whole reason it is a distributed file rather than a local one, and
//! putting any of it here would turn a typo into a chain split.
//!
//! The same reasoning `crates/governance/src/limits.rs` uses: the safest way to stop a
//! value being set is for there to be no way to name it.
//!
//! # Precedence
//!
//! ```text
//! command-line flag  >  config.toml  >  compiled default
//! ```
//!
//! A flag beats the file so an operator can override one setting for one run
//! without editing state that outlives the run — which is what people actually
//! do at three in the morning.
//!
//! # Unknown keys are an error
//!
//! `deny_unknown_fields` on every struct. A typo like `p2p_prot = 30333` would
//! otherwise be silently ignored and the node would listen on the default,
//! which is the failure mode where the config *looks* applied and is not.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{NodeError, Result};

/// A node's operational configuration.
///
/// Every field has a compiled default, so an absent file and an empty file both
/// mean "the defaults", and a node started with neither behaves exactly as it
/// did before this module existed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NodeConfig {
    /// Networking.
    pub network: NetworkConfig,
    /// JSON-RPC server.
    pub rpc: RpcConfig,
    /// Storage engine tuning.
    pub storage: StorageConfig,
}

/// Networking settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NetworkConfig {
    /// libp2p TCP listen port.
    pub p2p_port: u16,
    /// Peer multiaddrs to dial at startup.
    pub bootnodes: Vec<String>,
    /// Dual-KEM transport policy: `off`, `preferred`, or `required`.
    ///
    /// Operational rather than consensus: it decides which transport protocols
    /// this node offers, and a node that offers fewer simply connects to fewer
    /// peers. It cannot make two nodes disagree about a block.
    pub dual_kem: String,
}

/// JSON-RPC settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RpcConfig {
    /// Listen address.
    ///
    /// # Bind this to localhost
    ///
    /// The node's JSON-RPC has no authentication and serves
    /// `get_mining_candidate` and `submit_block`. `maya-api-gateway` exists to
    /// be the public surface, with a default-deny allowlist that excludes both.
    /// The default here is `127.0.0.1`, and `deploy_bootstrap.sh` firewalls the
    /// port regardless.
    pub listen: SocketAddr,
    /// Requests per second permitted from one peer address.
    ///
    /// Zero disables the limiter. See [`crate::rpc::limit`].
    pub rate_limit_per_second: u32,
    /// Requests one peer may burst above the sustained rate.
    pub rate_limit_burst: u32,
}

/// Storage engine tuning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StorageConfig {
    /// RocksDB block cache, in mebibytes.
    ///
    /// The read cache. Larger keeps more of the account trie hot; the working
    /// set is a property of the chain's size and the node's traffic, so this is
    /// a knob rather than a constant.
    pub block_cache_mib: usize,
    /// RocksDB write buffer, in mebibytes.
    ///
    /// Memtable size before a flush. Larger means fewer, bigger SSTs and less
    /// write amplification, at the cost of that much resident memory per
    /// column family.
    pub write_buffer_mib: usize,
    /// Files RocksDB may keep open.
    ///
    /// `-1` lets RocksDB keep every file open, which is fastest and needs
    /// `LimitNOFILE` raised to match — the systemd units set it.
    pub max_open_files: i32,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            p2p_port: 30333,
            bootnodes: Vec::new(),
            // Off, matching `DualKemPolicy::default()`. The three conditions
            // for changing this are in `docs/pq-transport.md`.
            dual_kem: "off".to_string(),
        }
    }
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            // Localhost, not 0.0.0.0. An unauthenticated RPC that binds every
            // interface by default is a node that is exposed the moment it is
            // started outside a container network.
            listen: "127.0.0.1:8545".parse().expect("valid default address"),
            // 50/s sustained with a burst of 100. Chosen against what a
            // legitimate client does: a wallet polls a balance and submits a
            // transaction, an explorer walks blocks at a few per second. Fifty
            // is generous for both and far below what it takes to keep a node
            // busy answering instead of validating.
            rate_limit_per_second: 50,
            rate_limit_burst: 100,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            // 512 MiB and 64 MiB. RocksDB's own defaults are 8 MiB and 64 MiB,
            // and 8 MiB of block cache on a chain-state database means almost
            // every read reaches the disk. These are the smallest values that
            // are not obviously wrong; a large node should raise them.
            block_cache_mib: 512,
            write_buffer_mib: 64,
            max_open_files: -1,
        }
    }
}

impl NodeConfig {
    /// Reads a configuration file.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if the file cannot be read or does not parse,
    /// including for an unknown key — see the module documentation.
    pub fn from_path(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| NodeError::Decode(format!("reading {}: {e}", path.display())))?;
        Self::from_toml(&text)
    }

    /// Parses a configuration from TOML text.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] with the parser's message, which names the line.
    pub fn from_toml(text: &str) -> Result<Self> {
        let config: Self =
            toml::from_str(text).map_err(|e| NodeError::Decode(format!("parsing config: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Reads `path` if it exists, otherwise returns the defaults.
    ///
    /// Absent is not an error: a node with no configuration file is a node
    /// running the defaults, which is a supported and common deployment.
    ///
    /// # Errors
    ///
    /// Propagates a parse failure. A file that exists and is wrong is an error,
    /// because the operator meant something by it.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if path.exists() {
            Self::from_path(path)
        } else {
            Ok(Self::default())
        }
    }

    /// Checks the values that a type cannot.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] describing the field and the bound it broke.
    pub fn validate(&self) -> Result<()> {
        if self.network.p2p_port == 0 {
            return Err(NodeError::Decode(
                "network.p2p_port must not be 0; port 0 asks the OS to choose one, \
                 which makes a node unreachable at the address its peers were told"
                    .to_string(),
            ));
        }

        // A burst below the sustained rate is almost certainly a mistake: it
        // makes the limiter stricter than its own documented rate and produces
        // rejections an operator cannot explain from the numbers they set.
        if self.rpc.rate_limit_per_second > 0
            && self.rpc.rate_limit_burst < self.rpc.rate_limit_per_second
        {
            return Err(NodeError::Decode(format!(
                "rpc.rate_limit_burst ({}) is below rpc.rate_limit_per_second ({}); \
                 the burst is the bucket's capacity and cannot be smaller than one \
                 second of refill",
                self.rpc.rate_limit_burst, self.rpc.rate_limit_per_second
            )));
        }

        if self.storage.write_buffer_mib == 0 {
            return Err(NodeError::Decode(
                "storage.write_buffer_mib must not be 0".to_string(),
            ));
        }

        crate::network::pq::dual::DualKemPolicy::from_str_checked(&self.network.dual_kem)?;

        Ok(())
    }

    /// The dual-KEM policy this configuration names.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for an unrecognised policy name.
    pub fn dual_kem_policy(&self) -> Result<crate::network::pq::dual::DualKemPolicy> {
        crate::network::pq::dual::DualKemPolicy::from_str_checked(&self.network.dual_kem)
    }

    /// A commented template, for `deploy_bootstrap.sh` to write.
    ///
    /// Generated from the defaults rather than kept as a string constant, so a
    /// changed default cannot leave a stale example on disk claiming otherwise.
    #[must_use]
    pub fn template() -> String {
        let defaults = Self::default();
        format!(
            "# Maya2C node configuration.\n\
             #\n\
             # Operational settings only. Consensus rules — proof-of-work\n\
             # verification, the difficulty floor, and the DAG activation height —\n\
             # come from genesis.json, which every node on a network shares and\n\
             # diffs against the ceremony's COMMITMENT.txt. There is no field here\n\
             # that can change them, deliberately: a per-host consensus override\n\
             # turns a typo into a chain split.\n\
             #\n\
             # A command-line flag overrides anything set here.\n\
             # An unknown key is an error, not a warning.\n\
             \n\
             [network]\n\
             p2p_port = {}\n\
             bootnodes = []\n\
             # off | preferred | required. See docs/pq-transport.md before changing.\n\
             dual_kem = \"{}\"\n\
             \n\
             [rpc]\n\
             # Localhost. This interface has no authentication and serves the miner\n\
             # methods; maya-api-gateway is the public surface.\n\
             listen = \"{}\"\n\
             rate_limit_per_second = {}\n\
             rate_limit_burst = {}\n\
             \n\
             [storage]\n\
             block_cache_mib = {}\n\
             write_buffer_mib = {}\n\
             # -1 keeps every file open; raise LimitNOFILE to match.\n\
             max_open_files = {}\n",
            defaults.network.p2p_port,
            defaults.network.dual_kem,
            defaults.rpc.listen,
            defaults.rpc.rate_limit_per_second,
            defaults.rpc.rate_limit_burst,
            defaults.storage.block_cache_mib,
            defaults.storage.write_buffer_mib,
            defaults.storage.max_open_files,
        )
    }
}

/// Where a node looks for its configuration when no `--config` is given.
#[must_use]
pub fn default_path() -> PathBuf {
    PathBuf::from("/etc/maya2c/config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults() {
        // Absent and empty must both mean "defaults", or adding this module
        // would have changed the behaviour of every existing deployment.
        assert_eq!(
            NodeConfig::from_toml("").expect("empty parses"),
            NodeConfig::default()
        );
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let config = NodeConfig::from_toml("[network]\np2p_port = 40404\n").expect("parses");
        assert_eq!(config.network.p2p_port, 40404);
        assert_eq!(config.rpc.listen, NodeConfig::default().rpc.listen);
        assert_eq!(
            config.storage.block_cache_mib,
            NodeConfig::default().storage.block_cache_mib
        );
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        // The failure this guards: a typo that leaves the node on the default
        // while the operator believes their setting was applied.
        let error = NodeConfig::from_toml("[network]\np2p_prot = 30333\n")
            .expect_err("a typo must not be ignored");
        assert!(
            format!("{error}").contains("p2p_prot"),
            "the error must name the offending key: {error}"
        );
    }

    #[test]
    fn an_unknown_section_is_an_error() {
        assert!(NodeConfig::from_toml("[consensus]\nverify_pow = false\n").is_err());
    }

    #[test]
    fn there_is_no_way_to_express_a_consensus_change() {
        // The property this module is built around, asserted rather than
        // described. Every one of these is a real `ChainConfig` field; none may
        // be reachable from a file.
        for attempt in [
            "verify_pow = false\n",
            "[consensus]\nverify_pow = false\n",
            "[chain]\npow_limit = \"00\"\n",
            "[network]\nverify_pow = false\n",
            "[dag]\nactivation_height = 0\n",
        ] {
            assert!(
                NodeConfig::from_toml(attempt).is_err(),
                "a config file must not be able to set consensus: {attempt:?}"
            );
        }
    }

    #[test]
    fn a_zero_p2p_port_is_refused() {
        let error = NodeConfig::from_toml("[network]\np2p_port = 0\n").expect_err("port 0");
        assert!(format!("{error}").contains("p2p_port"));
    }

    #[test]
    fn a_burst_below_the_sustained_rate_is_refused() {
        // Stricter than the documented rate, and the rejections it causes
        // cannot be explained from the numbers the operator wrote.
        let error =
            NodeConfig::from_toml("[rpc]\nrate_limit_per_second = 50\nrate_limit_burst = 10\n")
                .expect_err("burst below rate");
        assert!(format!("{error}").contains("burst"));
    }

    #[test]
    fn a_zero_rate_disables_the_limiter_and_ignores_the_burst() {
        // Zero is "off", so the burst relationship does not apply.
        let config =
            NodeConfig::from_toml("[rpc]\nrate_limit_per_second = 0\nrate_limit_burst = 0\n")
                .expect("zero disables");
        assert_eq!(config.rpc.rate_limit_per_second, 0);
    }

    #[test]
    fn an_unknown_dual_kem_policy_is_refused_at_parse_time() {
        // Caught when the file is read, not when the first peer connects.
        assert!(NodeConfig::from_toml("[network]\ndual_kem = \"maybe\"\n").is_err());
        assert!(NodeConfig::from_toml("[network]\ndual_kem = \"required\"\n").is_ok());
    }

    #[test]
    fn the_rpc_default_is_localhost() {
        // An unauthenticated interface that binds every address by default is
        // a node exposed the moment it runs outside a container network.
        assert!(NodeConfig::default().rpc.listen.ip().is_loopback());
    }

    #[test]
    fn the_template_parses_and_round_trips_to_the_defaults() {
        // The template is generated from the defaults, so it must parse back to
        // them. If it ever did not, `deploy_bootstrap.sh` would write a file
        // that silently changed the node's behaviour.
        let parsed = NodeConfig::from_toml(&NodeConfig::template()).expect("template parses");
        assert_eq!(parsed, NodeConfig::default());
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_a_broken_one_is_an_error() {
        let dir = std::env::temp_dir().join("maya-config-test");
        let _ = std::fs::create_dir_all(&dir);

        let absent = dir.join("does-not-exist.toml");
        let _ = std::fs::remove_file(&absent);
        assert_eq!(
            NodeConfig::load_or_default(&absent).expect("absent is fine"),
            NodeConfig::default()
        );

        let broken = dir.join("broken.toml");
        std::fs::write(&broken, "[network]\np2p_prot = 1\n").expect("write");
        assert!(
            NodeConfig::load_or_default(&broken).is_err(),
            "a file that exists and is wrong is an error: the operator meant something by it"
        );
        let _ = std::fs::remove_file(&broken);
    }
}
