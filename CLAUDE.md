# Maya2C

Post-quantum L1 blockchain node in Rust (edition 2024, `rust-version = 1.88`).
Root package `custom-l1-node` + 29 workspace members, one lockfile, one `target/`.

## Token Discipline (read first)

This repo is large: `target/` is 356 GB and `wallet-gui/ui/target` is ~4,978
*tracked* files. Unbounded reads and unfiltered command output are the dominant
cost here, not model reasoning. The rules below are mechanical, not stylistic.

**Reading code — escalate, never start at the top:**

1. `skel <file>` — declaration-only skeleton with line numbers (~90% fewer
   tokens than the file; measured 19,819 -> 1,857 chars on `chain.rs`).
2. `ctx <symbol> [path]` — `file:line` of a definition, no bodies.
3. `serena` symbol tools (`find_symbol`, `get_symbols_overview`,
   `find_referencing_symbols`) for cross-file work.
4. `peek <file> <start> [count]` or `Read` with `offset`/`limit` for the range
   that steps 1-3 identified.
5. A whole-file read only when the file *is* the task and is under ~200 lines.

Never re-read a file already in context. Never read a file back to confirm a
write that returned success.

**Searching:** `rg` for text, `ast-grep` / `sgr` for structure. Both honor
`.ignore` and `.gitignore`. Never `Get-ChildItem -Recurse` from the repo root.

**Editing:** targeted `Edit` / `replace_symbol_body`. Never rewrite a whole file
to change part of it. Show diffs, not restatements of the file.

**Command output:** every build/test/lint call goes through `condense` or the
`qb` / `qt` / `ql` wrappers. `gdf` for stat, `gdfp` for a bounded unified diff,
`gst` / `glog` for git. An unfiltered `cargo` invocation is a defect.

**Responses:** terse and fragment-based. Lead with `file:line`. No preamble, no
restating the question, no summarizing output already shown.

Helpers live in `~/.claude/shell/token-helpers.ps1` and
`token-helpers-bounded.ps1` (`.sh` equivalents for bash), auto-loaded from the
PowerShell profile and `~/.bashrc`.

## Workspace Map

| Member | Role |
|---|---|
| *(root)* `custom-l1-node` | Node daemon: consensus, chain, p2p, RPC, metrics |
| `ledger-math` | All `u64` credit/debit/nonce math. Kani-verifiable — keep RocksDB out |
| `crypto-pq` | SLH-DSA instantiation. Must stay the monomorphizing crate |
| `zk-privacy` | Groth16 shielded joinsplits |
| `vm` | Wasm contract execution |
| `l2-flash` | L2 settlement |
| `wallet`, `wallet-gui/core` | CLI + GUI wallet |
| `explorer` | Chain explorer |
| `cuda-miner` | GPU miner. `cuda` feature OFF by default so CI stays green without a CUDA toolkit |
| `stratum-v2` | Pool protocol, no chain dependency — fuzzable in isolation |
| `pool-service` | Daemon joining SV2 to chain types (PPLNS, payouts) |
| `dex` | Constant-product curve + order book + batch clearing. Kani-verifiable — keep it dependency-free |
| `vrf` | RFC 9381 EC-VRF behind the randomness beacon. Pinned to the RFC's test vectors |
| `governance` | Proposal lifecycle, vote tally, and the bounds a proposal may never escape. Kani-verifiable — dependency-free |
| `fee-market` | EIP-1559 base fee over serialized **bytes** (there is no block gas), 80/20 burn/treasury split, supply cap. Research branch: `FeeConfig::DISABLED` (activation `u64::MAX`), called by nothing in `src/` — `tests/fee_market_tests.rs` checks both. Dependency-free for Kani |
| `faucet` | Testnet faucet. Funded key behind a public endpoint; the per-IP/per-address limiter and daily cap are the whole of what bounds a drain |
| `telemetry` | Network telemetry. `server`/`client` feature split so a miner links no web framework; the always-on half builds for `wasm32` |
| `custody-mpc` | Threshold custody of a chain key: dealerless Pedersen VSS, ML-KEM-sealed shares, quorum signing. Links no chain types — `tests/custody_parity_tests.rs` pins its derivation to `crypto::hybrid` |
| `zkml` | Verifies halo2 (KZG/BN254) proofs of quantized-classifier inference for `host_verify_zkml_proof`. Hand-written circuit, verifier only. **Dark** (`ZKML_ACTIVATION_HEIGHT = u64::MAX`), not post-quantum, SRS from a public seed |
| `zkml-prover` | Off-chain half of `zkml`: ONNX import via `tract-onnx`, key generation, proving. Its own crate, not a feature, so the node's graph cannot reach tract; `rust-version = 1.91` because tract's patched releases need it, and nothing the node links depends on it |
| `dashboard/` | Leptos browser page. **Not a workspace member** — CSR Leptos only runs on `wasm32`. Built with `trunk` |

## Critical Invariants

1. **`ledger-math` must never gain a C/C++ dependency.** Kani compiles a crate
   with its full dependency graph; pulling in RocksDB breaks model checking.
2. **`crypto-pq` must remain the crate that instantiates `slh-dsa`.** Measured:
   4468 ms → 140 ms signing when the *instantiating* crate is optimized rather
   than the generic one. Moving the instantiation moves the optimization.
3. **`cuda-miner`'s `cuda` feature stays default-off** so
   `cargo build --workspace` works on GPU-less runners.
4. **`fips204` compiles only the `ml-dsa-65` parameter set.** A compiled-in set
   is a set someone can select by accident.
5. **`[profile.dev.package.*]` overrides are load-bearing, not tuning.** Without
   them `cargo test` reads as hung, not slow. Do not "clean them up".
6. **`dex` must stay dependency-free**, for the same reason as `ledger-math`.
   The visible cost is that it cannot hash, so pair and share-asset identifiers
   are derived in `custom-l1-node` and passed in as opaque bytes. That is the
   price of the boundary, not an oversight to tidy up.
7. **A trade that merely *loses* is a no-op, never an `Err`.** A missed slippage
   bound, a lost arbitrage race, a batch the pool cannot price: nonce advances,
   nothing moves. A failing transaction fails its whole block here, so making
   any of these an error hands every trader a way to void a block. See
   `docs/dex.md`; `tests/dex_tests.rs` pins it.
8. **Every trading record's prior value goes in the undo journal.** A reorg that
   left a pool holding the abandoned chain's reserves is not a detectable
   corruption — it is two plausible numbers that go on quoting a price. The same
   applies to the oracle's `o:` records.
9. **Oracle freshness is measured in block height, never in timestamps.**
   `chain.rs` reads `header.timestamp` only for difficulty retargeting; there is
   no future-drift bound and no median-time-past, so a miner may write any
   `u64`. A timestamp-based freshness check would read as safety and provide
   none. See `docs/oracle.md`.
10. **The VRF suite octet is `0x03`.** `0x04` is `…-SHA512-ELL2`, the same curve
    and hash under a different hash-to-curve map. Using it produces proofs that
    are internally consistent, pass every round-trip test, and match no other
    implementation on earth. `vrf/tests/rfc9381_vectors.rs` is what catches it —
    do not "simplify" those vectors away.
11. **The oracle is optional and absent by default.** A genesis file without an
    `oracle` section produces no `o:` records and the state root the chain would
    have had without the subsystem. Introducing the chain's only trusted party
    is a decision somebody writes down.
12. **Governance must not be able to make governance unsafe.** The quorum floor,
    the approval floor, the minimum voting period, and the minimum timelock are
    compiled into `governance/src/limits.rs`, appear in no `ParameterKey`, and
    are reachable by no transaction. Every governable value carries a hard range
    checked *twice* — when proposed and again when executed, because a release
    between the two could have tightened it.
13. **No governance key's value is a program.** Native code is never fetched from
    chain state and run. A rule change either moves a number or flips between
    two implementations the binary already ships, as `crypto/dag/registry.rs`
    already does with `activation_height`. See `docs/governance.md`.
14. **Nothing on the telemetry dashboard is verified.** Every figure but
    difficulty is an unauthenticated claim. Heights and propagation are medians
    so one liar cannot set them, reports replace rather than accumulate, and
    claims outside a hard bound are rejected rather than clamped — a clamped
    report is a number the reporter never sent. See `docs/telemetry.md`.
15. **The telemetry map is country-granular and has a floor.** No address is
    stored, no coordinate exists anywhere in the crate, and a country with
    fewer than `MIN_REPORTERS` is folded into `ZZ`. One miner in a small
    country is an individual, not aggregate data. `MIN_REPORTERS` is a
    compiled-in constant and no configuration key, because tuning it to 1 to
    "see more detail" is the failure it prevents.
16. **The faucet's two rate-limit buckets are independent.** Keying on the
    `(IP, address)` pair is not a limit: keypairs are free, so one IP with a
    thousand fresh addresses is a thousand payouts. Both buckets must clear,
    and both are consumed only if both pass. The daily cap, not the limiter, is
    what bounds a distributed drain. See `docs/faucet.md`.
17. **A governed value is read from state, never from a `const`.** The constants
    that remain (`MAX_FILLS_PER_BLOCK`, `DEFAULT_PROTOCOL_FEE_BPS`, …) are the
    parameter table's *defaults*. Reading one directly at a call site silently
    un-governs that rule.
18. **`custody-mpc` reconstructs the vault key in one place, and that is the
    design, not a defect to fix quietly.** A Maya2C signature is a hybrid pair
    and both halves must verify; there is no threshold construction for
    SLH-DSA at all, and `fips204` exposes nothing that decomposes into partial
    ML-DSA signatures. So the crate protects the 32-byte *chain key* — which
    `signing_key_from_seed` expands into both halves — rather than thresholding
    either signature. The combiner holding the key for the length of one
    signature is the whole cost, it is stated at the top of `src/lib.rs`, and
    anything that quietly relaxes it (a "partial signature" API, a second
    combiner, caching a reconstructed seed) breaks the only claim the crate
    makes. See `docs/custody-mpc.md`.
19. **Every reconstruction is checked against the vault's commitment before a
    key is derived from it.** Interpolating from too few shares does not fail —
    it returns a different secret, silently. `vss::check_opening` is what turns
    a short quorum, a corrupted safe, or an inconsistent dealer into an error
    instead of a signature under a key that owns nothing. It is the reason
    `interpolate_opening` returns the blinding factor alongside the secret, and
    the reason there is no public way to obtain one without the other.
20. **No ONNX runtime on the consensus path.** The node depends on `maya-zkml`,
    which is the verifier only; `tract-onnx` lives in `maya-zkml-prover`, which
    the node takes as a dev-dependency and nothing more. A separate crate rather
    than a feature, because a feature can be switched on by any crate in the
    graph through unification and a crate the node does not depend on cannot.
    Most ONNX models are floating point, and a float in a consensus rule is a rounding
    mode two validators can disagree on. Verification checks a proof; nothing
    in a block runs a model. See `docs/zkml.md`.
21. **A host function that does native work charges fuel for it, first.** Gas
    is wasmtime fuel and cannot see native work, so `host_verify_zkml_proof`
    charges a *measured* price (`vm/src/zkml.rs`, calibrated by
    `vm/tests/fuel_calibration_tests.rs` and `zkml-prover/benches/verify.rs`) before
    it reads a byte, and traps out-of-fuel before the verifier runs. There is
    deliberately no tensor host function: guest wasm is priced exactly by the
    fuel meter, and a hand-set per-MAC price would be consensus-critical and
    wrong on some machine.
22. **zkML stays dark until its SRS is real and gas is capped.** The SRS in
    `zkml/src/srs.rs` is derived from a public seed, so anyone can forge proofs;
    and no cap bounds a call's `gas_limit`, so a fuel price bounds nothing
    absolutely. `ZKML_ACTIVATION_HEIGHT` is `u64::MAX`, and
    `state::zkml::check_setup` refuses mainnet the moment it is anything else
    while `SRS_IS_TRUSTED` is false.
23. **Every constraint in `zkml/src/circuit.rs` has a test that fails without
    it.** Negative tests hand the circuit a lie that is *consistent* — everything
    downstream recomputed — so only the guard under test can refuse it. A lie
    left inconsistent is caught by some other constraint, and the test then
    passes with its own guard deleted; that happened, and the mutation sweep in
    `docs/zkml.md` is how it was found. Changing the circuit means re-running
    that sweep.

## Build & Test

Never run unfiltered cargo output — use `qb` / `qt` / `ql`, or pipe through
`condense`.

```powershell
qb                                              # cargo build --workspace
qt                                              # cargo nextest run --workspace
ql                                              # cargo clippy --all-targets
cargo llvm-cov --workspace --summary-only       # 80% floor
bash scripts/doc_coverage.sh --check            # doc coverage (90% floor) + no broken doc links
cargo deny check                                # deny.toml is committed
cargo audit
```

First build after a clean is long — the `opt-level = 3` dev overrides mean the
crypto and arkworks stacks compile optimized even in debug.

## MCP Servers

Project scope, `.mcp.json`. Trimmed to four: every server's tool schemas ship in
the system prompt on every turn, so a server that duplicates a built-in is a
permanent tax.

| Server | Use it for |
|---|---|
| `serena` | **Primary code navigation.** rust-analyzer-backed symbol find/edit. Its file-read, shell, and memory tools are excluded in `.serena/project.yml` — they duplicate built-ins |
| `context7` | Live crate/API docs — check before assuming a crate's surface |
| `fetch` | Web retrieval (RFCs, FIPS specs) |
| `headroom` | Context compression (`headroom mcp serve`) |

Disabled deliberately: `filesystem` (duplicates Read/Write/Edit/Glob), `git`
(duplicates Bash git, allowlisted in `.claude/settings.json`), `memory`
(duplicates `codebase-memory-mcp` and the file memory under
`~/.claude/projects/`), `sequential-thinking` (duplicates built-in extended
thinking). Restore from the backup named in `.claude/settings.local.json` history
if ever needed.

User scope: `codebase-memory-mcp`.

**Graphify is a skill, not an MCP server** — invoke with `/graphify`.

## Ignore Rules

`.ignore` at the repo root is the real exclusion file: the Claude Code binary
references `.ignore`, `.rgignore`, and `.gitignore`, and contains **no**
reference to `.claudeignore` — the `.claudeignore` that used to live here was
inert and has been removed. `.gitignore` cannot exclude *tracked* paths, which is
why `.ignore` carries `wallet-gui/ui/target` and `target-contracts`.
`.claude/settings.json` adds `permissions.deny` as a hard backstop.

## Branding

`logo-assets/` is the source of truth and is never edited in place. Three
commands deploy it; the two scripts take `--check` so drift is a failure rather
than a discovery:

```powershell
python scripts/deploy_brand_assets.py    # favicons + wordmarks into each app
python scripts/make_og_card.py           # the 1200x630 social card
cargo tauri icon logo-assets/print/HighRes-Square-2000_2000x2000.png `
  -o wallet-gui/src-tauri/icons          # the desktop icon set
```

Two pack defects are corrected on copy, not propagated: `site.webmanifest` ships
an empty `name`/`short_name`, and `paste-in-head.html` hardcodes root-absolute
paths that only suit the explorer.

Reference checking differs per surface: `dashboard/` and `wallet-gui/ui/` fail
`trunk build` on a missing `data-trunk` dir, then `scripts/check_brand_refs.py`
walks each built `dist/`; `explorer/` has no build step, so
`tests/server_tests.rs` asks the running server for every path its rendered HTML
names. The explorer resolves `--assets` against its working directory, logs that
directory at startup, and warns loudly when it is absent — a `ServeDir` over a
missing path would 404 every icon and look like a browser problem.

## Caches

Dependency caches live on **D:** (`UV_CACHE_DIR=D:\Caches\uv_cache`,
`NPM_CONFIG_CACHE=D:\Caches\npm_cache`, set at user scope and pinned again in
`.mcp.json`). Do not create parallel cache dirs. Known deviation: `CARGO_HOME`
is still `C:\Users\EricW\.cargo` — relocating it means re-downloading the
registry and re-installing every cargo binary, left alone deliberately.

## Workflow Rules

1. Navigation order: `skel` / `ctx` -> `serena` symbols -> `ast-grep` -> bounded
   `Read`. Whole-file reads last.
2. Run `/graphify` after major structural changes.
3. Keep files modular — 200-400 lines typical, 800 max.
4. Consensus, crypto, and ledger changes get a `rust-reviewer` +
   `security-reviewer` pass before commit.
