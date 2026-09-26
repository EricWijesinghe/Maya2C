//! `cargo xtask readiness` — regenerates READINESS.md (Master Prompt 11 §5).
//!
//! One row per mainnet-core component (ADR-016). Every cell is either a link
//! to evidence **that exists on disk now** or the word GAP. Nothing is typed
//! in by hand, so a cell cannot claim evidence that was deleted or never
//! written: the command checks each path when it runs. `--check` exits
//! non-zero if READINESS.md is stale.

use std::fmt::Write as _;
use std::path::Path;

/// Evidence candidates per column; the first that exists is linked.
struct Component {
    name: &'static str,
    owner: &'static str,
    spec: &'static [&'static str],
    vectors: &'static [&'static str],
    tests: &'static [&'static str],
    fuzz: &'static [&'static str],
    formal: &'static [&'static str],
    bench: &'static [&'static str],
    runbook: &'static [&'static str],
}

const COMPONENTS: &[Component] = &[
    Component {
        name: "Crypto: ML-DSA / SLH-DSA / hybrid",
        owner: "crypto-pq",
        spec: &["spec/crypto.md"],
        vectors: &["spec/tests/crypto_kat.json", "crates/crypto-pq/tests/vectors/acvp"],
        tests: &["crates/crypto-pq/tests/acvp_tests.rs"],
        fuzz: &["fuzz/fuzz_targets/tx_decode.rs"],
        formal: &[],
        bench: &["reports/crypto-bench.md"],
        runbook: &["docs/runbooks/key-compromise.md"],
    },
    Component {
        name: "Crypto: ML-KEM + hybrid handshake",
        owner: "crypto-pq, node::network::pq",
        spec: &["spec/crypto.md"],
        vectors: &["crates/crypto-pq/tests/kem_kat_tests.rs"],
        tests: &["crates/crypto-pq/tests/kem_kat_tests.rs"],
        fuzz: &[],
        formal: &[],
        bench: &["reports/crypto-bench.md"],
        runbook: &["docs/runbooks/peer-starvation.md"],
    },
    Component {
        name: "Types and encoding",
        owner: "node::core",
        spec: &["spec/encoding.md"],
        vectors: &["spec/tests/encoding.json"],
        tests: &["crates/node/tests/malleability_tests.rs", "crates/node/tests/payload_tests.rs"],
        fuzz: &["fuzz/fuzz_targets/tx_decode.rs"],
        formal: &[],
        bench: &[],
        runbook: &[],
    },
    Component {
        name: "State transition",
        owner: "node::state, ledger-math",
        spec: &["spec/state-transition.md"],
        vectors: &["spec/tests/state_transitions.json"],
        tests: &["crates/node/tests/state_tests.rs", "crates/node/tests/exploit_replays.rs"],
        fuzz: &["fuzz/fuzz_targets/block_decode.rs", "fuzz/fuzz_targets/account_decode.rs"],
        formal: &["formal/lean/Maya2C/Supply.lean"],
        bench: &[],
        runbook: &["docs/runbooks/corrupted-state.md"],
    },
    Component {
        name: "Fee market",
        owner: "fee-market",
        spec: &["spec/fees.md"],
        vectors: &["spec/tests/fees.json", "formal/lean/vectors/fee_split.txt"],
        tests: &["crates/fee-market/tests/lean_differential.rs"],
        fuzz: &[],
        formal: &["formal/lean/Maya2C/FeeSplit.lean", "formal/z3/fee_market.py"],
        bench: &["reports/18-economics.md"],
        runbook: &[],
    },
    Component {
        name: "Consensus: dag-bft",
        owner: "dag-bft (not wired into node)",
        spec: &["spec/consensus.md"],
        vectors: &["spec/tests/commit_rule.json"],
        tests: &["crates/dag-bft/tests/modes_sim.rs"],
        fuzz: &[],
        formal: &[],
        bench: &["reports/04-consensus.md"],
        runbook: &["docs/runbooks/chain-halt.md"],
    },
    Component {
        name: "Mempool",
        owner: "node",
        spec: &["spec/mempool.md"],
        vectors: &[],
        tests: &["crates/node/tests/network_tests.rs"],
        fuzz: &[],
        formal: &[],
        bench: &[],
        runbook: &["docs/runbooks/mempool-flood.md"],
    },
    Component {
        name: "Staking and slashing",
        owner: "(not built)",
        spec: &["spec/staking.md"],
        vectors: &[],
        tests: &["crates/staking/tests/slashing_tests.rs"],
        fuzz: &[],
        formal: &[],
        bench: &[],
        runbook: &["docs/runbooks/validator-slashed.md"],
    },
    Component {
        name: "WASM VM (Cranelift + cache)",
        owner: "vm",
        spec: &["spec/vm.md"],
        vectors: &[],
        tests: &["crates/vm/tests/vm_tests.rs", "crates/vm/tests/tier_differential_tests.rs"],
        fuzz: &[],
        formal: &[],
        bench: &["docs/vm-module-cache.md", "reports/05-vm.md"],
        runbook: &[],
    },
    Component {
        name: "Governance",
        owner: "governance",
        spec: &["spec/governance.md"],
        vectors: &[],
        tests: &["crates/node/tests/governance_tests.rs", "crates/governance/tests/limits_tests.rs"],
        fuzz: &[],
        formal: &["crates/governance/src/proofs.rs"],
        bench: &[],
        runbook: &["docs/runbooks/stuck-upgrade.md"],
    },
    Component {
        name: "p2p (TCP/QUIC)",
        owner: "node::network",
        spec: &["spec/networking.md"],
        vectors: &[],
        tests: &["crates/node/tests/network_tests.rs", "crates/node/tests/pq_transport_tests.rs"],
        fuzz: &[],
        formal: &[],
        bench: &[],
        runbook: &["docs/runbooks/peer-starvation.md"],
    },
    Component {
        name: "RPC",
        owner: "node::rpc, api-gateway",
        spec: &["spec/rpc.md"],
        vectors: &[],
        tests: &["crates/node/tests/rpc_tests.rs", "crates/api-gateway/tests/gateway_tests.rs"],
        fuzz: &[],
        formal: &[],
        bench: &[],
        runbook: &["docs/runbooks/rpc-overload.md"],
    },
    Component {
        name: "Multisig custody",
        owner: "crypto-pq::multisig",
        spec: &["spec/transactions.md"],
        vectors: &[],
        tests: &["crates/crypto-pq/tests/multisig_tests.rs"],
        fuzz: &[],
        formal: &[],
        bench: &[],
        runbook: &["docs/runbooks/key-compromise.md"],
    },
];

fn cell(root: &Path, candidates: &[&str]) -> String {
    candidates
        .iter()
        .find(|p| root.join(p).exists())
        .map_or_else(|| "GAP".to_string(), |p| format!("[{p}]({p})"))
}

pub fn render(root: &Path) -> (String, usize, usize) {
    let mut out = String::new();
    let (mut evidence, mut gaps) = (0usize, 0usize);
    let _ = writeln!(out, "# Production readiness scorecard\n");
    let _ = writeln!(
        out,
        "Generated by `cargo xtask readiness`; do not edit by hand. One row per mainnet-core \
         component (ADR-016). A cell links evidence that existed when the command ran, or says \
         GAP. \"Externally audited\" is GAP everywhere: no external audit has happened.\n"
    );
    let _ = writeln!(
        out,
        "| Component | Owner | Spec | Conformance vectors | Unit + property tests | Fuzzed | Formal | Benchmarked | Externally audited | Runbook |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|");
    for c in COMPONENTS {
        let cells = [
            cell(root, c.spec),
            cell(root, c.vectors),
            cell(root, c.tests),
            cell(root, c.fuzz),
            cell(root, c.formal),
            cell(root, c.bench),
            "GAP".to_string(),
            cell(root, c.runbook),
        ];
        for x in &cells {
            if x == "GAP" {
                gaps += 1;
            } else {
                evidence += 1;
            }
        }
        let _ = writeln!(out, "| {} | {} | {} |", c.name, c.owner, cells.join(" | "));
    }
    let _ = writeln!(out, "\n**{evidence} cells with evidence, {gaps} gaps.**");
    (out, evidence, gaps)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let (text, evidence, gaps) = render(&root);
    let path = root.join("READINESS.md");
    if args.iter().any(|a| a == "--check") {
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current != text {
            return Err("READINESS.md is stale; run `cargo xtask readiness`".into());
        }
    } else {
        std::fs::write(&path, &text).map_err(|e| e.to_string())?;
    }
    println!("readiness: {evidence} cells with evidence, {gaps} gaps ({} components)", COMPONENTS.len());
    Ok(())
}
