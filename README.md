<p align="center">
  <img src="logo-assets/website/Header-Logo_250x100.png" width="250" height="100" alt="Maya2C"/>
</p>

<p align="center">
  A post-quantum layer-1 blockchain node, in Rust.
</p>

---

## The short version

Every transaction carries **two** signatures — ML-DSA-65 (FIPS 204) and
SLH-DSA-SHA2-128s (FIPS 205) — and both must verify. That is 11,165 bytes per
transaction, which is the price of not betting the chain on one lattice
assumption. Transport is libp2p with an ML-KEM-768 layer over Noise, and an
optional HQC-192 second KEM behind a flag.

**Mainnet is deliberately blocked.** `SETUP_IS_TRUSTED` is `false`: the Groth16
parameters come from a reproducible test setup, not a ceremony, and six separate
guards refuse a value-bearing chain id. See
[docs/mainnet-readiness.md](docs/mainnet-readiness.md) — that block is the first
thing to read before anything else here matters.

## Workspace

| Member | Role |
|---|---|
| *(root)* `custom-l1-node` | Node daemon: consensus, chain, p2p, RPC, metrics |
| `ledger-math` | All `u64` credit/debit/nonce math. Kani-verifiable — no C/C++ in its graph |
| `crypto-pq` | SLH-DSA instantiation. Must stay the monomorphizing crate |
| `zk-privacy` | Groth16 shielded joinsplits |
| `vm` | Wasm contract execution |
| `l2-flash` | L2 settlement |
| `wallet`, `wallet-gui/` | CLI wallet, and a Tauri desktop wallet whose keys never leave the Rust core |
| `explorer` | Server-rendered chain explorer |
| `cuda-miner`, `wgpu-miner` | GPU miners. Both GPU features are default-off so CI builds without an adapter |
| `stratum-v2`, `pool-service` | Pool protocol, and the daemon joining it to chain types |
| `dex` | Constant-product curve, order book matcher, batch clearing. Dependency-free |
| `vrf` | RFC 9381 EC-VRF behind the randomness beacon |
| `governance` | Proposal lifecycle and the bounds a proposal may never escape |
| `faucet` | Testnet faucet. A hot wallet on a public endpoint — see its doc before deploying it |
| `telemetry` | Network telemetry collector, split into `server` and `client` halves |
| `dashboard/` | Leptos browser page. Not a workspace member: CSR Leptos only runs on `wasm32` |

## Build

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
cd dashboard      && trunk build --release   # telemetry dashboard
cd wallet-gui/ui  && trunk build --release   # wallet frontend
```

## Documentation

[docs/README.md](docs/README.md) indexes the full set. The ones to read first:

- [mainnet-readiness.md](docs/mainnet-readiness.md) — why mainnet is blocked
- [launch-checklist.md](docs/launch-checklist.md) — what remains before a
  value-bearing launch
- [hybrid-signatures.md](docs/hybrid-signatures.md) — the two-signature scheme
- [security-audit.md](docs/security-audit.md)

## Branding

Logo assets live in [`logo-assets/`](logo-assets/) and are the source of truth.
Two scripts deploy them, and both take `--check` so drift is detectable rather
than discovered:

```bash
python scripts/deploy_brand_assets.py        # favicons and wordmarks into each app
python scripts/make_og_card.py               # the 1200x630 social card
cargo tauri icon logo-assets/print/HighRes-Square-2000_2000x2000.png \
  -o wallet-gui/src-tauri/icons               # the desktop icon set
```

## Status

This is pre-launch software on a chain that refuses to be worth anything. Read
`CLAUDE.md` for the invariants that are load-bearing before changing consensus,
crypto, or ledger code.
