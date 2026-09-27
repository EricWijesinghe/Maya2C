//! The post-quantum claim for zero knowledge, checked rather than asserted.
//!
//! ADR-008 removed every pairing-based proof system from the tree: Groth16 over
//! BLS12-381 (`zk-privacy`) and halo2/KZG over BN254 (`zkml`). A claim like that
//! decays silently — one new dependency that pulls `ark-groth16` back in and the
//! shielded pool's soundness rests on discrete log again, with nothing failing.
//! So the dependency graph itself is the test.

use std::process::Command;

use maya_zk_stark::Proof;
use maya_zk_stark::pool::note::SpendingKey;
use maya_zk_stark::pool::tree::CommitmentTree;
use maya_zk_stark::pool::{self, wallet};

/// Crates whose presence would mean a pairing, a trusted setup, or both.
const FORBIDDEN: &[&str] = &[
    "ark-groth16",
    "ark-bn254",
    "ark-bls12-381",
    "ark-snark",
    "halo2_proofs",
    "halo2-axiom",
    "halo2curves",
    "halo2-base",
    "bls12_381",
    "bellman",
    "snark-verifier",
];

/// Pairing curves the EVM's own precompiles need (ecAdd/ecMul/ecPairing over
/// BN254, EIP-2537 over BLS12-381), pulled in by `revm-precompile` for
/// `crates/multivm` (Master Prompt 5, RESEARCH). They are Ethereum's rules for
/// EVM contracts, which ADR-023 labels classical-security; no Maya proof
/// system uses them. Allowed only on that path — the next test checks it.
const EVM_COMPAT_ONLY: &[&str] = &["ark-bn254", "ark-bls12-381"];
/// The only packages allowed to depend on an [`EVM_COMPAT_ONLY`] crate.
const EVM_PATH: &[&str] = &[
    "revm-precompile",
    "revm",
    "revm-handler",
    "revm-inspector",
    "maya-multivm",
];

/// Every package the host build of the workspace compiles, across every edge
/// kind, one per line.
///
/// Host target only: `--target all` needs every other platform's crates on
/// disk, and a test must not reach the network to fetch them. The lockfile
/// test below covers the other targets.
fn workspace_packages() -> String {
    cargo_tree(&["--workspace", "--exclude", "maya-multivm"])
}

/// `cargo tree` over the workspace with `extra` arguments, one package per line.
fn cargo_tree(extra: &[&str]) -> String {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .arg("tree")
        .args(extra)
        .args([
            "--manifest-path",
            manifest,
            "--edges",
            "normal,build,dev",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--offline",
        ])
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

#[test]
fn no_pairing_or_trusted_setup_crate_is_in_the_workspace_graph() {
    let packages = workspace_packages();
    let names: Vec<&str> = packages
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    // A graph this small would mean the command printed nothing useful, and
    // an empty list contains no forbidden crate.
    assert!(
        names.len() > 100,
        "cargo tree listed only {} packages",
        names.len()
    );
    assert!(
        names.contains(&"p3-uni-stark"),
        "the STARK prover is in the graph"
    );

    let found: Vec<&&str> = FORBIDDEN.iter().filter(|f| names.contains(f)).collect();
    assert!(
        found.is_empty(),
        "pairing-based crates re-entered the graph: {found:?}"
    );
}

#[test]
fn no_pairing_or_trusted_setup_crate_is_in_the_lockfile() {
    // The lockfile resolves every target at once, so a dependency gated to a
    // platform `cargo tree` did not look at still shows up here.
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
        .expect("workspace Cargo.lock");
    let names: Vec<&str> = lock
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .collect();
    assert!(names.contains(&"p3-uni-stark"), "the lockfile was parsed");

    let found: Vec<&&str> = FORBIDDEN
        .iter()
        .filter(|f| names.contains(f) && !EVM_COMPAT_ONLY.contains(f))
        .collect();
    assert!(
        found.is_empty(),
        "pairing-based crates are in Cargo.lock: {found:?}"
    );
}

/// The two EVM curves are in the lockfile since `crates/multivm` (2026-09-27);
/// this pins that nothing but the EVM engine reaches them. Every other
/// workspace member is covered by the graph test above, which excludes only
/// `maya-multivm`.
#[test]
fn evm_pairing_curves_are_reached_only_through_the_evm_engine() {
    for curve in EVM_COMPAT_ONLY {
        let dependents = cargo_tree(&["--workspace", "--invert", curve]);
        let names: Vec<&str> = dependents
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| name != curve)
            .collect();
        let stray: Vec<&&str> = names.iter().filter(|n| !EVM_PATH.contains(n)).collect();
        assert!(
            stray.is_empty(),
            "{curve} is reached outside the EVM engine via {stray:?}"
        );
    }
}

#[test]
fn a_shielded_proof_verifies_and_a_flipped_byte_does_not() {
    let owner = SpendingKey::random().expect("key");
    let anchor = CommitmentTree::default().root();
    let built = wallet::shield(500, 5, owner.address(), anchor).expect("build");
    let (proof, public) = pool::prove(&built.witness).expect("prove");
    assert_eq!(pool::verify(&proof, &public), Ok(()));
    assert!(proof.as_bytes().len() <= pool::MAX_PROOF_BYTES);

    // Hash-based soundness means any bit of the transcript matters; flip one
    // in the middle, where the FRI query openings live.
    let mut bytes = proof.as_bytes().to_vec();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    assert!(pool::verify(&Proof::from_bytes(bytes), &public).is_err());

    // And the public inputs are bound: the same proof for a different amount.
    let mut inflated = public;
    inflated.public_in += 1;
    assert!(pool::verify(&proof, &inflated).is_err());
}
