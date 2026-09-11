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
4. `Read` with `offset`/`limit` for the range that steps 1-3 identified. Use
   `Read` rather than `peek`/`sed -n` through a shell: it costs the same, and it
   is what `Edit` checks for before it will change a file.
5. A whole-file read only when the file *is* the task and is under ~200 lines.

Never re-read a file already in context. Never read a file back to confirm a
write that returned success.

**Read the region before you edit it.** Session history showed 1,377
`Edit`/`Write` calls against 188 `Read`s: most reading went through the shell,
and 548 of the writes were whole files. So:

- `Write` is for **new** files only. To change an existing file, `Read` the
  range and then `Edit` it.
- A scripted rewrite (`python`, `sed -i`) is only for *mechanical* changes
  that the compiler checks afterwards, such as adding a struct field at every
  site `cargo check` reports. Take the list of target sites from the tool's own
  output, never from a guess, and run the check straight afterwards.
- Before editing a function you have not seen this session, read all of it,
  not just the lines you are changing. An edit that makes sense in isolation
  can break an invariant stated three lines above it.

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
24. **A block's id and proof of work cover its transactions, and its declared
    state root is checked.** `BlockHeader::tx_root` (bytes 112..144, after the
    nonce so `NONCE_RANGE` never moved) is a `state::merkle` root over
    `transaction_leaf(txid)`. `Chain::insert_block` calls `check_tx_root`
    *first*, ahead of the duplicate check and before anything is stored: a
    mismatched body filed under an honest id would turn the genuine block into
    a `Duplicate`, which is censorship by one relay. `apply_block_journaled` —
    the chain's only apply path — refuses a `state_root` that execution does not
    produce. Block producers take both roots from `Chain::candidate_block` and
    never from `state_root()`, which is the *pre*-block root. Before this, one
    block id could carry two transaction lists (`tests/chaos_simulator.rs`
    replays that attack). Do not add an unchecked apply path to `Chain`.
25. **Every persisted consensus record is under the state root; everything
    else is on an explicit local-only list.** `state::commitments` holds the
    lists:
    - `RECORD_LAYERS`, the one source for the generic-record prefixes;
    - `committed_prefixes()`;
    - `LOCAL_ONLY_PREFIXES` (`undo:`, `blk:`).

    Until 2026-09-12 contract code, contract storage, the nullifier set, and the
    shielded pool's anchor window and balance were all outside the root.
    Nothing checked them, a snapshot could forge them, and a reorg did not even
    restore contract storage. A new prefix goes into those lists and into a
    layer, and the undo journal records its prior value. Otherwise
    `uncovered_keys()` fails the subsystem's tests, and the `write_overlay`
    debug assertion fails any test that writes a stray record.

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
cargo fuzz list                                 # fuzz/ targets (nightly; decoders + SV2 frames)
cargo machete                                   # unused-dependency *candidates* — heuristic, verify each
```

`cargo machete` flags `maya-sdk-ffi -> maya-crypto-pq`, a dependency that
exists for invariant 2 rather than for any `use`. Check every hit against the
invariants before removing it.

First build after a clean is long — the `opt-level = 3` dev overrides mean the
crypto and arkworks stacks compile optimized even in debug.

## MCP Servers

Project scope, `.mcp.json`, enabled in `.claude/settings.local.json`. Tool
search defers full schemas, but every server still puts its tool *names* and its
instructions block in the prompt on every turn. So a server nobody calls, or a
tool nobody calls, is a permanent tax. The set below was cut to what 18 sessions
of transcripts show being used (2026-09-11), and each server has one job so that
two of them are never competing for the same question:

| Question | Server and tool |
|---|---|
| Where is `X` defined? What is in this file? | `serena` `find_symbol` / `get_symbols_overview` (rust-analyzer: exact) |
| Who uses `X`? Rename `X` everywhere. | `serena` `find_referencing_symbols` / `rename_symbol` |
| What calls what, across crates? What is the shape of this subsystem? | `codebase-memory-mcp` `trace_path` / `get_architecture` / `search_graph` |
| A crate's API, before depending on it or calling a part of it this repo does not use yet | `context7` `resolve-library-id` then `query-docs` (the `docs-lookup` agent runs on it) |
| One known URL (an RFC, a FIPS spec, docs.rs) | `fetch` |

- **`serena`**: its file-read, shell, memory and text-insertion tools are
  excluded in `.serena/project.yml`, because the built-ins already do those jobs.
- **`codebase-memory-mcp`** (0.10.8): `.mcp.json` overrides the user-scope entry
  with `--tool-profile=scout`, which cuts it from 15 tools to 7 and from 24.9K to
  13.8K schema characters.
  - Re-indexing is not in the scout profile. Re-index with
    `codebase-memory-mcp cli index_repository '{"repo_path":"D:/Maya2C","mode":"full"}'`.
  - `claude mcp list` warns that the server is defined in two scopes. That is the
    override working, not a fault.
  - Version 0.9.0 silently skipped files.

Disabled deliberately:
- **Project servers, via `disabledMcpjsonServers`:**
  - `filesystem` duplicates Read/Write/Edit/Glob.
  - `git` duplicates Bash git, which is allowlisted in `.claude/settings.json`.
  - `memory` duplicates `codebase-memory-mcp` and the file memory under
    `~/.claude/projects/`.
  - `sequential-thinking` duplicates built-in extended thinking.
  - `headroom` was never called. Its `headroom_compress` takes the text as an
    argument, so that text is already in context before anything is compressed.
    It cannot save tokens from inside the conversation.
- **User servers and connectors, via `disabledMcpServers`:**
  - `rustrover` was never called, and adds about 40 tool names (SQL, database,
    run-configuration tools) that serena and the shell already cover. It is
    also only live while the IDE runs.
  - The claude.ai connectors `Shopify` and `Viewmax` add about 80 tool names
    between them and have nothing to do with a blockchain.

All of these are reversible: remove the name from the list. The pre-trim configs
are in `~/.claude/backups/mcp-trim-20260911/`.

**Research tooling is shell-side, not MCP**, so it costs nothing until it's used:

| Tool | Use it for |
|---|---|
| `agent-reach` skill (`~/.claude/skills/agent-reach/`) | Multi-source research. It routes to the tools below. The CLI is pinned to upstream `Panniantong/Agent-Reach@da5044d`; do not run `check-update` |
| `mcporter call 'exa.web_search_exa(query: "...", numResults: 5)'` | Semantic web search: papers, advisories, standards. Exa is configured in `~/.mcporter/mcporter.json` |
| `yt-dlp --write-auto-sub --skip-download` | Conference-talk transcripts. `~/.config/yt-dlp/config` sets `--js-runtimes node` |
| `gh` | Issues, PRs, releases, `gh search code`. **Needs `gh auth login` once** |

Social channels (X, Reddit, …) are deliberately unconfigured: they need the
user's browser cookies. Agent-Reach's own MCP server exposes only `get_status`,
so it is not registered.

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
