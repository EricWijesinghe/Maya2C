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

/// Every package the host build of the workspace compiles, across every edge
/// kind, one per line.
///
/// Host target only: `--target all` needs every other platform's crates on
/// disk, and a test must not reach the network to fetch them. The lockfile
/// test below covers the other targets.
fn workspace_packages() -> String {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--workspace",
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

    let found: Vec<&&str> = FORBIDDEN.iter().filter(|f| names.contains(f)).collect();
    assert!(
        found.is_empty(),
        "pairing-based crates are in Cargo.lock: {found:?}"
    );
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
