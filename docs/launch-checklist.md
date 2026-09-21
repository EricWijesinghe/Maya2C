# Launch checklist

What was built, what was verified, and what still blocks a value-bearing
launch.

---

## The block is still in place, and nothing here lifted it

`docs/mainnet-readiness.md §1`: the shielded pool's circuit has had no
independent audit. One missing constraint in the joinsplit AIR lets anyone mint
shielded value that no supply audit reveals — including the supply endpoints
this repo serves to aggregators.

**Four independent guards refuse a value-bearing chain id.** A fifth was added
by this work:

| Guard | Behaviour |
|---|---|
| `crates/zk-stark/src/pool/mod.rs` | `CIRCUIT_IS_AUDITED = false` |
| `bins/maya2c-node/src/main.rs` `VALUE_BEARING_CHAINS` | node exits at startup |
| `infra/terraform/modules/*/variables.tf` | `terraform plan` fails |
| `crates/zk-stark/src/pool/tests.rs` | test pins the flag false |
| **`bins/genesis-ceremony/src/main.rs`** | **refuses to mint the genesis file** |

The ceremony binary is the newest and, for this purpose, the most important: a
genesis file is the one artefact that cannot be revised afterwards, so the tool
that mints it is the wrong place to be permissive.

Lifting the block needs a multi-party ceremony with at least one honest
participant destroying their contribution, parameters committed and
independently verified, and only then the flag flipped. That is calendar time
and external participants.

---

## 1. Genesis ceremony

`cargo run --bin genesis-ceremony -- --chain-id maya-genesis-rc1 --supply ... `

Produces root keys, a funded DAO treasury, a `genesis.json`, and a commitment
sheet operators diff against each other.

**Secret material never reaches stdout.** Root keys are written to `*.secret`
files (mode `0600` where the platform has modes) and the terminal sees only
public halves, addresses, and the state root. A key echoed to a terminal is a
key in a scrollback buffer, a multiplexer capture, and whatever CI collected the
job log — and a root key leaked at generation cannot be rotated, because it is
what the chain's identity was built on.

### Verified run

```
chain id       maya-genesis-rc1
total supply   21000000000
treasury       4200000000 units (2000 bps)
roots          3 x 5600000000
state root     45e2811b46e12ca72a0e16b426f81232ab0cbfb6ea2236e9223846e2eff9f15b
genesis block  23fbae9752388b17890b1aaaf4fffd67ab291233df2d9b0fc874c1767e15c99e
```

3 × 5.6B + 4.2B = 21B exactly.

**That genesis block id is stale; the state root is not.** It was recorded
before the header gained `tx_root` (2026-09-11). The header is part of the id,
so the same `genesis.json` now has a different genesis block id, while its state
root is unchanged. The ceremony generates fresh root keys, so this run cannot be
replayed. The id to lock is the one the ceremony prints when it is run for real.

### The treasury declares its share and is checked against it

`TreasuryGenesis::share_bps` is written by the author and validated against
what the file actually implies. A treasury meant to be 20% and actually 100% is
a validation error rather than a discovery. The ceiling is
`MAX_TREASURY_SHARE_BPS = 5 000` — half — because a treasury holding most of
the supply is not a treasury, it is the chain.

**A chain without a treasury has exactly the state root it always had.** That
is the property `treasury_tests.rs` leads with: a new optional field that
changed the root of every existing chain would silently fork every network
already running. A pre-treasury `genesis.json` still parses and still produces
the identical root.

### "Genesis DAG root" does not exist

`crypto/dag` is the Ethash-style memory-hard proof-of-work **dataset**, not a
block graph. `maya-blockgraph` is a research branch no consensus path reaches.
What is locked is the **state root** and the **genesis block id**, both above.

---

## 2. Technical reference

`cargo run -p maya-docgen -- --out docs/reference.tex`

188 documented modules across 18 crates, 252 KB of LaTeX, **16 sections
carrying a status banner.**

### It is a reference, not a whitepaper

A whitepaper argues: it states a problem, proposes a design, and defends the
choices — and it is written by someone who decided what to leave out. This
scrapes `//!` documentation and arranges it. The result is thorough and
accurate and useful, and calling it a whitepaper would oversell it to exactly
the audience least able to tell.

### It marks research branches instead of laundering them

Status is **detected from each module's own words**, not decided by the
generator. A module that says "research branch" or "no consensus path reaches
this" gets a banner; the generator has no independent knowledge of what ships
and inventing one would mean two things to keep in sync.

Two corrections were needed during the work:

1. **A false positive.** `crates/api-gateway/src/graphql.rs` is shipped, and its
   documentation explains what it declines to expose by naming a research
   branch. A whole-document scan labelled the module itself a research branch —
   telling a reader the gateway's GraphQL layer was not real. Detection now
   reads only the `# Status` section or the opening lines.
2. **A false negative.** Only each crate's `lib.rs` was flagged, so a reader
   landing on `crates/blockgraph/schedule.rs` saw a detailed scheduler description with
   nothing saying no block reaches it. Crate-level status now propagates.

### No PDF was produced

Neither `pdflatex` nor `tectonic` is installed on the build host. The `.tex` is
generated and unbuilt; the escaping is unit-tested (`latex_specials_are_escaped`
and the code-fence cases), but nothing has confirmed the document compiles.
**Build it before circulating it.**

---

## 3. Deployment

`infra/k8s/deploy.yaml` — 10 documents: namespace, two ConfigMaps, genesis node,
10 seed nodes, two services, two PodDisruptionBudgets, a default-deny
NetworkPolicy.

### They are seed nodes, not validators

Maya2C is proof-of-work. There is no validator set, no stake, no quorum, and no
permissioning — these eleven pods have no more authority than a laptop joining
tomorrow. What they provide is bootstrap connectivity and reachable RPC.
"Validator" implies a permissioning model this chain does not have, and a
launch document is where that misunderstanding takes root.

### Placeholders that must be replaced

- `REPLACE_WITH_DIGEST` — an image **digest**, not a tag. A fleet that pulled
  `:latest` at different moments runs different consensus rules while reporting
  the same version.
- `REPLACE_WITH_CEREMONY_GENESIS_JSON`
- `REPLACE_WITH_COMMITMENT_STATE_ROOT` / `_BLOCK_ID`

`kustomize` base and the six region overlays remain the maintained path for
ongoing operation. This file is a single-document launch topology; the two must
be kept consistent.

### Two public-facing services ship alongside the node

Both terminate untrusted HTTP, and neither is part of consensus.

| Service | Runs on mainnet? | Doc |
|---|---|---|
| `faucet` | **No.** Refuses at construction | `docs/faucet.md` |
| `telemetry` | Yes; it holds no key and moves no value | `docs/telemetry.md` |

The faucet is a hot wallet with a public endpoint, which is why the chain
refusal is a sixth guard beside `CIRCUIT_IS_AUDITED`, the node's startup check,
both terraform module sets, the ceremony, and the wallet composer. It is
refused at construction rather than per request, so a misconfigured deployment
fails to start instead of failing on the first request somebody is watching.

Both read a client's address from a proxy header only when explicitly told a
proxy is there (`MAYA_FAUCET_TRUST_PROXY`, `MAYA_TELEMETRY_TRUST_PROXY`), and
both default to off. Turning either on when the port is also reachable directly
means the direct path has no limit and no geolocation at all — the setting is a
claim about the network topology, not a preference.

Neither is in `infra/k8s/deploy.yaml`. They are optional services, and adding them to
the launch topology is a decision somebody makes rather than a default.

### The telemetry dashboard is not evidence

Every figure it serves except difficulty is an unauthenticated claim by
whoever reported it. It is a picture of what nodes say. Quoting a hash rate
from it as a network measurement is the specific misuse `docs/telemetry.md`
exists to prevent, and the caveat is on the page itself for the same reason.

---

## 4. Safety checks

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets` | exit 0, no first-party warnings |
| `cargo nextest run --workspace` | see below |
| `scripts/check-unsafe.sh` | **ok — every first-party unsafe carries a SAFETY comment** |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok |
| `cargo audit` | 2 vulnerabilities, 5 warnings — see below |

### "Zero unsafe across the entire repository" is not a reachable state

The tree links RocksDB (C++ FFI), libp2p, and `hqc-kem`'s PCLMULQDQ intrinsic.
Zero unsafe in the dependency graph is not achievable, and a gate set at zero
would be a gate nobody could pass.

What is gated is the half this project controls, and it is clean. First-party
`unsafe` inventory:

```
hal/cuda-miner/src/gpu.rs           7
crates/node/benches/hybrid_footprint.rs     5
apps/wallet-gui/ui/src/bridge.rs     1
crates/node/src/state/contracts.rs          1
crates/node/src/crypto/dag/dataset.rs       1
bins/pool-service/src/treasury.rs    1
crates/governance/src/proposal.rs      1
crates/governance/src/lib.rs           1
crates/blockgraph/src/lib.rs           1
```

Every one carries a `// SAFETY:` comment. `cargo geiger` was **not** run:
`scripts/check-unsafe.sh` documents why it is a survey and not a gate, and it is
not installed here.

### The audit/deny discrepancy, resolved

`cargo audit` reports RUSTSEC-2026-0118 and RUSTSEC-2026-0119 against
`hickory-proto`, and RUSTSEC-2026-0253 against `lru`. `cargo deny check`
reports advisories ok. Both are correct, and the difference matters:

- **`cargo audit` reads `Cargo.lock`.** Every crate the resolver ever
  considered is in there.
- **`cargo deny` reads the resolved dependency graph.** Only crates actually
  built appear.

`hickory-proto` and `lru` are in the lockfile because libp2p declares them under
an optional `dns` feature that is **not enabled**. Confirmed empirically: after
a full workspace build, `target/debug/build` contains no `hickory` or `lru`
artifact, and `cargo deny list` does not list them.

**They are not compiled into any Maya2C binary.** The advisories are real and do
not apply to this build. Recorded here rather than silenced in `deny.toml` —
that file's `ignore = []` is deliberate, and an advisory accepted should be
accepted in a commit that says why.

The `chacha20` yanked warning and the `derivative` / `paste` /
`proc-macro-error2` unmaintained warnings are transitive and surfaced, not
fatal, per `deny.toml`'s `unmaintained = "workspace"`.

---

## Outstanding before a value-bearing launch

1. **Independent audit of the joinsplit AIR.** The block above. Everything
   else is downstream of it.
2. **Compile `docs/reference.tex`.** Generated, never built.
3. **Replace every placeholder in `infra/k8s/deploy.yaml`.**
4. **Move the ceremony's `*.secret` files to their custodians** and remove them
   from the generating host.
5. **Kotlin and Swift SDK bindings are generated but never compiled** — see
   `docs/sdk.md`.
6. **The dual-KEM transport stays `off`** until the three conditions in
   `docs/pq-transport.md` hold.
7. **The faucet must never be pointed at a value-bearing chain.** It refuses,
   but the deployment should not be the thing testing that.
8. **`cuda-miner` has no telemetry export.** Its `argon_blake_hash` is a
   single-hash function with no loop of its own to meter; the loop lives in
   whatever calls it. `wgpu-miner --benchmark` is the only path that currently
   produces a real GPU hash rate, and it is a benchmark rather than mining —
   there is no header from a node, no target to meet, and nothing is submitted.
9. **The header and state-root changes are an unreleased hard fork.** Blocks
   now commit to their transactions (`tx_root`, header 112 -> 144 bytes), the
   chain checks every declared `state_root`, and the root folds contract state,
   the nullifier set and the whole shielded pool. Every node on a network runs
   the same build or splits, and the genesis block id recorded above predates
   it. See invariants 24 and 25.
10. **Pruning is off by default; Arweave upload is the one edge left open.**
    The IPFS path is verified against a real kubo daemon
    (`crates/archive/tests/kubo_live.rs`: import, pin, export, verify), and the CAR
    decoder's property is covered on every platform by a seeded randomized test
    as well as by the fuzz target, which still needs Linux or macOS. Arweave
    upload stays unimplemented on purpose: it spends AR per byte and cannot be
    tested without spending. See [pruning.md](pruning.md).
11. **The Ledger app is a feasibility spike, not a product.** Its Speculos suite
    ships unrun and no device or emulator exists here. See
    [ledger-feasibility.md](ledger-feasibility.md).
12. **`wgpu-miner` still has no work-distribution loop.** The meter is wired
   into `GpuMiner::mixes`, where the GPU work happens, so it is already correct
   when that loop lands — but no `wgpu-miner` process mines a block today.
