//! Workspace automation.
//!
//! Two commands, both of which exist because something went wrong without
//! them:
//!
//! - `disk` — `target/` reached 415 GB once and filled the volume to exactly
//!   zero bytes, at which point every build fails with `os error 112`, which
//!   names nothing about the real problem. It was 289 GB at the commit that
//!   added this crate.
//! - `coverage` — subsystem status lived in three prose documents and nothing
//!   checked that a subsystem claiming to work had a test. `features.toml` is
//!   the machine-readable half; this is what reads it.
//!
//! Run as `cargo xtask <command>` (the alias is in `.cargo/config.toml`).

mod attacknet;
mod bench_tps;
mod claims_check;
mod coverage;
mod devnet;
mod disk;
mod eco_metrics;
mod evidence;
mod genesis_check;
mod go_no_go;
mod guides_check;
mod hub_bundle;
mod localnet;
mod mesh_check;
mod mining_localnet;
mod pgo;
mod proving;
mod readiness;
mod release_check;
mod sdk_e2e;
mod site_integrity;
mod slo_check;
mod spec_coverage;
mod status;
mod sweep;
mod up;
mod verify_release;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => {
            usage();
            return ExitCode::FAILURE;
        }
    };

    let result = match cmd {
        "disk" => disk::run(rest),
        "coverage" => coverage::run(rest),
        "release-check" => release_check::run(rest),
        "readiness" => readiness::run(rest),
        "spec-coverage" => spec_coverage::run(rest),
        "slo-check" => slo_check::run(rest),
        "go-no-go" => go_no_go::run(rest),
        "genesis-check" => genesis_check::run(rest),
        "eco-metrics" => eco_metrics::run(rest),
        "claims-check" => claims_check::run(rest),
        "mesh-check" => mesh_check::run(rest),
        "sdk-e2e" => sdk_e2e::run(rest),
        "guides-check" => guides_check::run(rest),
        "pgo" => pgo::run(rest),
        "hub-bundle" => hub_bundle::run(rest),
        "bench-tps" => bench_tps::bench_tps(rest),
        "localnet" => localnet::localnet(rest),
        "mining-localnet" => mining_localnet::mining_localnet(rest),
        "proving" => proving::proving(rest),
        "attacknet" => attacknet::attacknet(rest),
        "up" => up::up(),
        "down" => up::down(),
        "status" => status::run(rest),
        "sweep" => sweep::run(rest),
        "verify-release" => verify_release::run(rest),
        "site-integrity" => site_integrity::run(rest),
        "help" | "--help" | "-h" => {
            usage();
            return ExitCode::SUCCESS;
        }
        other => Err(format!("unknown command `{other}`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "\
cargo xtask <command>

  disk [--check]        Report build-artifact size. `--check` exits non-zero
                        above the ceiling in xtask/src/disk.rs.
  coverage [--quiet]    Read features.toml, print the ledger, and fail if an
                        entry claims `working` or `verified` without naming a
                        test that exists.
  verify-release <dir> [--checksums-only]
                        Check a downloaded release: SHA256SUMS, cosign
                        signatures from the release workflow, and GitHub
                        build attestations (docs/verify-release.md).
  release-check [--force-sim]
                        Build maya2c-node --features production and prove no
                        SIM crate or forbidden feature is linked. --force-sim
                        forces a SIM feature in and must fail at the guard.
  readiness [--check]   Regenerate READINESS.md from evidence on disk;
                        --check fails if it is stale.
  spec-coverage [--strict]
                        List every spec/ rule and whether spec/tests/ has a
                        positive and a negative vector for it. --strict fails
                        on any consensus rule without both.
  slo-check             Fail if any SLO in docs/SLO.md lacks a registered
                        metric, a dashboard panel, an alert or a runbook.
  go-no-go              Compute every mainnet launch gate from evidence:
                        PASS (with its evidence), FAIL, or NEEDS HUMAN.
  genesis-check FILE    Mainnet genesis policy: >=7 validators, no operator above
                        20% of stake, stake-weighted, real difficulty.
  eco-metrics           Ecosystem metrics computable from this repository
                        (active developers, retention, contracts), with a
                        named gap for each one that needs chain data.
  claims-check          Fail if a public surface calls a feature \"first\",
                        \"only\" or \"unprecedented\" without citing a completed
                        docs/prior-art/ search that permits the claim.
  mesh-check            Run mesh-cli check:data against a one-validator
                        devnet and maya2c-mesh [--mesh-cli PATH]
                        [--timeout SECS] [--workdir DIR].
  sdk-e2e               Run the TypeScript SDK's live-node test through
                        maya2c-gateway against a one-validator devnet.
  guides-check          Fail if a command in docs/ names a package, test,
                        binary, xtask or maya2c command, or script that does
                        not exist.
  pgo                   Measure profile-guided optimisation on bft_tps:
                        baseline vs PGO build, medians of --runs runs.
  mining-localnet [--gpu] [--seconds N]
                        Four validators with block rewards in genesis and the
                        release miner on node 1: a share pays, payouts survive
                        one validator down, all nodes agree on balances.
  hub-bundle            Build the release miner and package the Maya2C Hub
                        with it: NSIS/MSI on Windows, deb/rpm/AppImage on
                        Linux (run in WSL).
  attacknet [--rounds N] [--weighted] [--beyond-retention]  Seven validators as processes, attacked:
                            crashes, a stolen key, wire garbage, RPC floods,
                            a long outage; checks no fork and recovery
  bench-tps [--senders N] [--txs N]  Finalized transfers/s on the 4-validator localnet
  localnet [--remote-signer]  Four maya2c-node validators as processes over
                        libp2p on 127.0.0.1: one chain, a transfer on all,
                        liveness with one killed, and its catch-up.
  up                    The whole ecosystem on this machine, left running:
                        four validators, the API gateway, a chat relay and a
                        funded devnet wallet. Prints the endpoints.
  down                  Stop what `up` started.
  status [--live]       Where the project stands, from evidence on disk: the
                        last sweep, the gap register, the ledger, STATE.md's
                        milestone and next tasks. Builds nothing unless --live.
  site-integrity sri <dist> | manifest <dist> <out> [--commit SHA]
                        | check <manifest> <live_dir>
                        Subresource integrity for the built site, a SHA-256
                        manifest of its served files, and the comparison of
                        fetched live bodies against that manifest.
  sweep [--clean] [STEP...]
                        Run every gate in order (fmt, unsafe, build, clippy,
                        nextest, ledger, spec, readiness, lint-debt,
                        doc-coverage, deny) and record reports/sweeps/."
    );
}

/// The workspace root, found by walking up from this crate's manifest.
///
/// `CARGO_MANIFEST_DIR` is set by cargo when it builds this binary, so the
/// answer does not depend on the directory the command was run from.
pub fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}

/// `(member directory, package name)` for every workspace member, read from
/// the manifests (no `cargo metadata`: these commands run before a build).
pub fn members(root: &std::path::Path) -> Result<Vec<(String, String)>, String> {
    #[derive(serde::Deserialize)]
    struct Root {
        workspace: Ws,
    }
    #[derive(serde::Deserialize)]
    struct Ws {
        members: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Member {
        package: Pkg,
    }
    #[derive(serde::Deserialize)]
    struct Pkg {
        name: String,
    }
    let text = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let manifest: Root = toml::from_str(&text).map_err(|e| e.to_string())?;
    manifest
        .workspace
        .members
        .into_iter()
        .map(|dir| {
            let text = std::fs::read_to_string(root.join(&dir).join("Cargo.toml"))
                .map_err(|e| format!("{dir}: {e}"))?;
            let m: Member = toml::from_str(&text).map_err(|e| format!("{dir}: {e}"))?;
            Ok((dir, m.package.name))
        })
        .collect()
}
