<p align="center">
  <a href="https://maya2c.dev">
    <img src="docs/brand/hero.svg" width="100%" alt="Maya2C: the post-quantum layer-1, built in Rust. Animated banner with the official emblem.">
  </a>
</p>

<p align="center">
  <b>A layer-1 blockchain in Rust. Every transaction is signed twice with post-quantum signatures, and both must verify.</b>
</p>

<p align="center">
  <a href="https://maya2c.dev"><b>maya2c.dev</b></a> &nbsp;·&nbsp;
  <a href="https://eric.maya2c.dev"><b>Built by Eric Wijesinghe: eric.maya2c.dev</b></a>
</p>

<p align="center">
  <a href="https://status.maya2c.dev"><img alt="testnet height" src="https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fstatus.maya2c.dev%2Fapi%2Fstatus&query=%24.height&label=testnet%20block&color=1fb5c2&style=for-the-badge&labelColor=0a161b"></a>
  <a href="https://status.maya2c.dev"><img alt="seconds since last block" src="https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fstatus.maya2c.dev%2Fapi%2Fstatus&query=%24.since_last_block&suffix=%20s%20ago&label=last%20block&color=45e0a0&style=for-the-badge&labelColor=0a161b"></a>
  <a href="LICENSE"><img alt="license" src="https://img.shields.io/badge/license-Apache%202.0-c9a86a?style=for-the-badge&labelColor=0a161b"></a>
  <img alt="Rust 1.88, edition 2024" src="https://img.shields.io/badge/rust-1.88%20%C2%B7%202024-e6b260?style=for-the-badge&logo=rust&labelColor=0a161b">
</p>

<p align="center">
  <a href="https://maya2c.dev/experience/"><b>✦ Experience it live</b></a> &nbsp;·&nbsp;
  <a href="https://maya2c.dev/guides/testnet/">Join the testnet</a> &nbsp;·&nbsp;
  <a href="https://explorer.maya2c.dev/">Explorer</a> &nbsp;·&nbsp;
  <a href="https://faucet.maya2c.dev/">Faucet</a> &nbsp;·&nbsp;
  <a href="https://status.maya2c.dev/">Status</a> &nbsp;·&nbsp;
  <a href="https://maya2c.dev/guides/validators/">Run a validator</a>
</p>

---

<p align="center"><img src="docs/brand/divider-1.svg" width="100%" alt=""></p>

## Run a node in one line

Paste one line and your computer joins the test network. No technical
background is needed. Steps for each OS:
**[maya2c.dev/guides/help-run](https://maya2c.dev/guides/help-run/)**.

| Your computer | Paste this |
|---|---|
| **Windows** (PowerShell) | `irm https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.ps1 \| iex` |
| **Mac** (Terminal) | `curl -fsSL https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.sh \| bash` |
| **Linux** | `curl -fsSL https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.sh \| bash` |

<p align="center"><img src="docs/brand/divider-2.svg" width="100%" alt=""></p>

## Two signatures per transaction

<p align="center"><img src="docs/brand/architecture.svg" width="100%" alt="Animated card: a transaction moves from wallet to API gateway to node, which verifies both signatures, then the WASM VM, DAG-BFT, and finality."></p>

```mermaid
flowchart LR
  classDef step fill:#0a161b,stroke:#1fb5c2,color:#edf3f2,stroke-width:2px
  classDef pq fill:#2a210f,stroke:#c9a86a,color:#f4ead6,stroke-width:2px
  W["🔐 Wallet<br/>signs the transaction"]:::step --> G["🛡️ API gateway<br/>default-deny allowlist"]:::step
  G --> N["⚙️ Node<br/>Transaction::verify"]:::step
  N --> S["✍️ ML-DSA-65 <b>and</b> SLH-DSA<br/>both must verify"]:::pq
  S --> V["🧩 WASM VM<br/>fee market · state root"]:::step
  V --> C["🧭 DAG-BFT<br/>Narwhal + Bullshark"]:::step
  C --> F["✅ Finalized<br/>state committed"]:::step
```

- Two NIST standards: **ML-DSA-65** (FIPS 204, lattice) and **SLH-DSA-SHA2-128s** (FIPS 205, hash-based). The chain does not rest on a single lattice assumption.
- The price is size: **11,165 bytes** of signature per transaction ([docs/hybrid-signatures.md](docs/hybrid-signatures.md)).
- Transport is libp2p, with an ML-KEM-768 layer over Noise.

**Mainnet is deliberately blocked.** `CIRCUIT_IS_AUDITED` is `false` at
[`crates/zk-stark/src/pool/mod.rs:36`](crates/zk-stark/src/pool/mod.rs#L36):
the shielded pool's joinsplit circuit has no independent audit, and six
separate guards refuse a value-bearing chain id. Read
[docs/mainnet-readiness.md](docs/mainnet-readiness.md) before anything else here.

<p align="center"><img src="docs/brand/divider-1.svg" width="100%" alt=""></p>

## What is real today

Counted from the repo, not from the roadmap. Status meanings are defined in
[docs/architecture-vision.md](docs/architecture-vision.md).

<p align="center"><img src="docs/brand/status-strip.svg" width="100%" alt="Status strip from features.toml: 154 subsystems, 61 verified, 72 working, 21 planned, 77 research, mainnet audit blocked."></p>

| Count | Value | Source |
|---|---|---|
| Subsystems in the ledger | **154** | [`features.toml`](features.toml), `[[subsystem]]` |
| …verified (named test exists) | **61** | same |
| …working | **72** | same |
| …planned (no code yet) | **21** | same |
| Of the 154, research (tested, no consensus caller) | **77** | same, `class = "RESEARCH"` |
| Register entries (the plan) | **164**: 50 verified, 114 planned | same, `[[feature]]` |
| Mainnet audit flag | `CIRCUIT_IS_AUDITED = false` | [`pool/mod.rs:36`](crates/zk-stark/src/pool/mod.rs#L36) |

`cargo xtask coverage` checks the ledger and fails when a claim has no test
behind it. Counts above are a plain read of the file at this commit.

## The short version

- **Consensus:** DAG-BFT finality, Narwhal + Bullshark, stake-weighted. The node side is in [`bins/maya2c-node/src/bft.rs`](bins/maya2c-node/src/bft.rs) ([ADR-027](docs/adr/ADR-027-dag-bft-in-the-node.md)).
- **Execution:** a Wasm VM with gas, a fee market, and a state root that a block must reproduce.
- **Money math:** `ledger-math` holds the credit, debit and nonce arithmetic, with no C/C++ in its dependency graph, so Kani can verify it.

## Architecture

```mermaid
flowchart TB
  classDef app fill:#0a161b,stroke:#1fb5c2,color:#edf3f2,stroke-width:2px
  classDef core fill:#06343a,stroke:#7eeaf0,color:#edf3f2,stroke-width:2px
  classDef pq fill:#2a210f,stroke:#c9a86a,color:#f4ead6,stroke-width:2px
  W["🔐 Wallet · Maya Chat · SDKs"]:::app --> G["🛡️ API gateway<br/>default-deny allowlist"]:::app
  G --> N["⚙️ Node: WASM VM · fee market · state"]:::core
  N <--> C["🧭 DAG-BFT consensus<br/>Narwhal + Bullshark · stake-weighted"]:::core
  C <--> P["🌐 libp2p · TCP + WebSocket<br/>ML-KEM-768 over Noise"]:::pq
  N --> S["✍️ Every transaction signed twice<br/>ML-DSA-65 + SLH-DSA"]:::pq
```

<details>
<summary><b>The core crates</b> (click to open)</summary>

| Member | Role |
|---|---|
| *(root)* `custom-l1-node` | Node daemon: consensus, chain, p2p, RPC, metrics |
| `ledger-math` | All `u64` credit/debit/nonce math. Kani-verifiable — no C/C++ in its graph |
| `crypto-pq` | SLH-DSA instantiation. Must stay the monomorphizing crate |
| `zk-stark` | Plonky3 STARKs: shielded joinsplits, credentials, sanctions proofs |
| `vm` | Wasm contract execution |
| `l2-flash` | L2 settlement |
| `wallet`, `apps/wallet-gui/` | CLI wallet, and a Tauri desktop wallet whose keys never leave the Rust core |
| `explorer` | Server-rendered chain explorer |
| `cuda-miner`, `wgpu-miner` | GPU miners. Both GPU features are default-off so CI builds without an adapter |
| `stratum-v2`, `pool-service` | Pool protocol, and the daemon joining it to chain types |
| `dex` | Constant-product curve, order book matcher, batch clearing. Dependency-free |
| `vrf` | RFC 9381 EC-VRF behind the randomness beacon |
| `governance` | Proposal lifecycle and the bounds a proposal may never escape |
| `faucet` | Testnet faucet. A hot wallet on a public endpoint — see its doc before deploying it |
| `telemetry` | Network telemetry collector, split into `server` and `client` halves |
| `light-client` | SPV header fork choice and state-proof verification |
| `mev` | Threshold-encrypted mempool: a miner orders transactions it cannot read |
| `custody-mpc` | Threshold custody of a chain key: dealerless VSS, ML-KEM-sealed shares, quorum signing |
| `archive` | CAR v1 (zstd) archives of pruned block batches: local dir, IPFS, Arweave |
| `api-gateway` | REST and GraphQL gateway, talking to a node over JSON-RPC |
| `sdk-ffi`, `sdk-wasm` | uniffi bindings, and browser bindings, over the hybrid signing primitives |
| `docgen` | Generates the LaTeX technical reference from module documentation |
| `apps/dashboard/` | Leptos browser page. Not a workspace member: CSR Leptos only runs on `wasm32` |

</details>

### Research branches

These crates are in the tree and tested, but **no consensus path calls them**. Each is
dark behind an activation height of `u64::MAX`, or is off-chain entirely. The
rationale for each is in
[docs/architecture-vision.md](docs/architecture-vision.md).

<details>
<summary><b>The research crates</b> (click to open)</summary>

| Member | Role |
|---|---|
| `fee-market`, `neural-gas-trainer` | EIP-1559 base fee over bytes, plus an integer neural gain inside a Kani-proved envelope, and the off-chain trainer for that gain |
| `blockgraph` | Narwhal/Tusk batch references and deterministic shard scheduling |
| `lattice-pow` | Lattice proof-of-useful-work (SVP) verification |
| `zkml`, `zkml-prover` | halo2 proofs of quantized-classifier inference: the verifier, and the off-chain prover |
| `rwa`, `identity` | Real-world assets and self-sovereign identity (DIDs, selective disclosure) |
| `iso20022` | ISO 20022 bank-rail messages bridged to payment intents |
| `radio-transport` | Off-grid LoRa framing, a duty-cycle governor, a fountain codec |
| `htlc-lattice`, `htlc-watcher` | Lattice HTLC atomic swaps (Maya2C↔Maya2C only), and the counterparty watcher |
| `stateless-core` | Sparse-Merkle account witnesses for stateless transfer verification |
| `confidential-ai` | Federated training with ML-KEM secure aggregation and integer differential privacy |
| `ebpf-net`, `hal/ebpf-net/common` | UDP block relay judged by an XDP program at the driver (Linux, `xdp` feature) |
| `threat-intel`, `threat-firewall` | Threat indicators built from verifiable gossip evidence, and the per-host nftables worker |
| `iot-anchor` | Hardware-anchored sensor identity: ML-DSA-65 device keys, PUF, TPM sealing, hash-chained telemetry batches. `no_std`; `hal/iot-firmware/` runs it on Cortex-M33 under QEMU |

</details>

Three more directories have their own workspaces and toolchains: `offsec-sandbox/`
(red-team fuzzing), `hal/ebpf-net/programs/` (the XDP program) and `fuzz/`.

## Quickstart

### Run the whole ecosystem locally

```sh
cargo xtask up      # 4 validators (DAG-BFT), API gateway, chat relay, funded wallet
cargo xtask down    # stop exactly what `up` started
```

`up` builds what it needs, waits until the validators agree on a chain and
every service answers, then prints the endpoints (also in
`target/up/up.json`). It is a local devnet: keys are generated fresh each
time and everything listens on 127.0.0.1 only. Run it in a terminal. On
Windows the services it leaves running inherit a piped stdout, so
`cargo xtask up | ...` waits until `down`; redirect to a file instead.

### Build and test

Requires Rust 1.88 (edition 2024).

```bash
cargo build --workspace
cargo nextest run --workspace      # nextest is the primary runner
cargo clippy --workspace --all-targets
cargo deny check
```

The first build after a clean is long: `[profile.dev.package.*]` overrides
compile the crypto and arkworks stacks optimized even in debug, which is
load-bearing rather than tuning — without them `cargo test` reads as hung.

### The browser frontends

Both are built with [`trunk`](https://trunkrs.dev) and live outside the Cargo
workspace's test surface:

```bash
cd apps/dashboard      && trunk build --release   # telemetry dashboard
cd apps/wallet-gui/ui  && trunk build --release   # wallet frontend
```

<p align="center"><img src="docs/brand/divider-2.svg" width="100%" alt=""></p>

## Support Maya2C

Maya2C is built in the open by one maintainer. Here is how to help:

| | |
|---|---|
| ⭐ **Star** | [Star the repo](https://github.com/EricWijesinghe/Maya2C) so more people find it |
| 🍴 **Fork** | [Fork it](https://github.com/EricWijesinghe/Maya2C/fork) and try the devnet on your own machine |
| 🛠️ **Contribute** | Read [CONTRIBUTING.md](CONTRIBUTING.md). Issues and proposals are welcome |
| 🌱 **Good first issues** | [Browse the label](https://github.com/EricWijesinghe/Maya2C/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) |
| 💬 **Talk** | [Discussions](https://github.com/EricWijesinghe/Maya2C/discussions) |
| ☕ **Donate** | Donation links are coming soon. None are live yet. |

Security reports go privately, never in public issues: see [SECURITY.md](SECURITY.md) for scope, response targets and safe harbour. Reporters are named in the [Security Hall of Fame](https://maya2c.dev/guides/security-hall-of-fame/). There are no cash rewards yet. Contributor badges are described on [Contributors & badges](https://maya2c.dev/guides/contributors/).

## Documentation

[docs/README.md](docs/README.md) indexes the full set; [docs/REPO-MAP.md](docs/REPO-MAP.md) says where every directory lives. Start with:

- [mainnet-readiness.md](docs/mainnet-readiness.md): why mainnet is blocked
- [launch-checklist.md](docs/launch-checklist.md): what remains before a value-bearing launch
- [hybrid-signatures.md](docs/hybrid-signatures.md): the two-signature scheme
- [security-audit.md](docs/security-audit.md)
- [architecture-vision.md](docs/architecture-vision.md): what each subsystem is, and its status

## Branding

Logo assets live in [`logo-assets/`](logo-assets/) and are the source of truth.
Two scripts deploy them, and both take `--check` so drift is detectable rather
than discovered:

```bash
python scripts/deploy_brand_assets.py        # favicons and wordmarks into each app
python scripts/make_og_card.py               # the 1200x630 social card
cargo tauri icon logo-assets/print/HighRes-Square-2000_2000x2000.png \
  -o apps/wallet-gui/src-tauri/icons               # the desktop icon set
```

The hero banner in `docs/brand/` is drawn from the official emblem in
`docs/site/public/brand/emblem.png`. The emblem is never redrawn.

## Status

This is pre-launch software on a chain that refuses to be worth anything. Read
`CLAUDE.md` for the invariants that are load-bearing before changing consensus,
crypto, or ledger code.

## Author

Eric Wijesinghe builds Maya2C. Portfolio: [eric.maya2c.dev](https://eric.maya2c.dev) · GitHub: [@EricWijesinghe](https://github.com/EricWijesinghe)

## License

Apache License 2.0: see [LICENSE](LICENSE) and [NOTICE](NOTICE). You may use,
modify and distribute this code, including commercially, under its terms,
which include an express patent grant.

The cryptography has **not** been independently audited. Mainnet is blocked
until it has been (see [docs/mainnet-readiness.md](docs/mainnet-readiness.md)).
