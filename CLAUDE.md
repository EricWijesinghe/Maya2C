# Maya2C

Post-quantum L1 blockchain node in Rust (edition 2024, `rust-version = 1.88`,
`nightly-2026-07-15`). A virtual workspace of 80 members plus ten crates
deliberately outside it; the node is `crates/node` — [ADR-001](docs/adr/ADR-001-workspace-layout.md).

## Identity and Mission

Maya2C is scoped as an **autonomous, post-quantum Layer-1 monolithic
ecosystem**: one chain that owns its cryptography, its execution, its transport,
and the hardware it attests, rather than a settlement layer delegating each of
those elsewhere. The stated target spans software, hardware, space
communications, bio-computing, and quantum physics.

What exists is not the target, and the difference is written down in three
places that must agree:

| Question | Read |
|---|---|
| What does this tree contain, and does it work? | `features.toml` — 164 register entries + 129 subsystems, gated by `cargo xtask coverage` |
| Why is each subsystem the way it is? | [docs/architecture-vision.md](docs/architecture-vision.md) — the authority for status |
| What is the build order? | [docs/trajectory.md](docs/trajectory.md) (a *plan*) and [PROGRESS.md](PROGRESS.md) (what is done) |

## Standing Orders

These apply to every task in this repository, without being restated.

1. **Never claim a test passed, a benchmark number, or "zero warnings" unless
   the real command output is in the report.** This has already gone wrong
   here: an inventory said "zero clippy warnings" because it counted summary
   lines instead of reading the run. There was one.
2. **Numbers in a brief are targets.** 1M TPS, 10× faster, sub-10 ms: measure
   and report the real figure. Never tune a benchmark toward a number fixed in
   advance, and never hard-code one. `crates/node/benches/algo_comparison.rs` states in its
   own output what it could not measure rather than leaving a silent gap.
3. **SIM and RESEARCH declare themselves.** A simulator says so in its logs,
   its docs and its CLI output. No simulator reaches a production path without
   an explicit feature flag, and no RESEARCH subsystem reaches consensus
   without an activation height somebody wrote down.
4. **Consensus-critical code has no floats, no `HashMap` iteration order, no
   wall-clock time, and no `unsafe` without a `// SAFETY:` comment and a test.**
   Invariants 9, 20, 24 and 28 are the specific cases; this is the general rule.
5. **Secrets are typed, zeroized, and never logged.** `zeroize` is already a
   dependency of every crate holding key material. A `Vec<u8>` that went
   through a serializer has already been copied.
6. **Nothing that costs money, touches a live server, or publishes a package
   runs without being asked first** — `terraform apply`, the deploy scripts,
   `cargo publish`, `npm publish`.
7. **Work in small steps.** After each: `cargo check`, run the affected tests,
   commit with a message that says *why*, tick `PROGRESS.md`. If context runs
   low, stop cleanly and write down where to resume.
8. **Every major design choice gets an ADR** in [docs/adr/](docs/adr/) — written
   when the decision is made, not afterwards.
9. **A finding that contradicts the brief is reported, not worked around.**
   Three of the four duplications the foundation brief asked to merge did not
   exist; saying so was the deliverable.
10. **No "first", "only" or "unprecedented"** in any document, UI or
   announcement unless `docs/prior-art/<feature>.md` records a dated search
   showing who else does something similar and exactly how Maya2C differs.
   Enforced: `cargo xtask claims-check` in CI.

## Production Standing Orders

From Master Prompt 11, with how each is enforced *today*:

- The mainnet binary is built with `--features production` — `cargo xtask release-check`.
- Every performance number carries git commit, hardware, OS, kernel and the
  exact command. **No `cargo xtask bench` exists yet**; `reports/12-baseline.md`
  is the manual record.
- "TPS" means signature-verified, executed, state-committed, finalized
  transactions per second with the mix stated. None has been measured: the
  node has no BFT finality (ADR-015).
- Consensus-critical changes need a spec update, conformance vectors
  (`cargo xtask spec-coverage`), an ADR if behaviour changes, and two human
  reviewers (a note; not enforced).
- No new core dependency without a `cargo vet` entry — **`cargo vet` is not set
  up**; `cargo deny` is the only supply-chain gate.
- CI fails on a >5% regression of tracked benchmarks — `scripts/bench_gate.py`,
  gated on a `BENCH_RUNNER` that does not exist, so it never runs.
- Secrets never enter the repo, logs or chat; anything that costs money or
  touches real servers needs `APPROVED: <step name>`.

## Token Discipline (read first)

This repo is large: build artifacts reached 336.5 GiB before the last
`cargo clean`, and `apps/wallet-gui/ui/target` is ~4,978 *tracked* files. Unbounded
reads and unfiltered command output are the dominant cost here, not model
reasoning. The rules below are mechanical, not stylistic.

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

80 members, and ten tracked crates that are **not**
members because they target a different architecture or must keep their
dependency graph away from the node's: `fuzz/`, `offsec-sandbox/`,
`hal/iot-firmware/`, `hal/ebpf-net/programs/`, `apps/dashboard/`, `apps/wallet-gui/ui/`,
`apps/wallet-gui/src-tauri/`, `apps/ledger-maya2c/`, `contracts/token-swap/`,
`contracts/nft-game/`. Seven of the ten cannot be built for the host at all, so
"the workspace builds" is a claim about 80 of 90 crates.

What each one is for, and why it is a separate crate:
[docs/workspace-map.md](docs/workspace-map.md). Membership itself comes from
`cargo metadata --no-deps`.

## Tiers, classes and the reality ledger

`features.toml` is the machine-readable answer to "does this work": 164 register entries and 129 subsystems;
`cargo xtask coverage` prints them and **fails** when one claims `working` or
`verified` without naming a test that exists. Schema and rationale:
[ADR-004](docs/adr/ADR-004-reality-ledger.md).

**Tier** — what a build compiles. `core` (crypto, state, consensus, VM, RPC,
p2p), `extended` (services, clients, SDKs, off-chain provers), `frontier`
(hardware, space, bio, and their simulators).

> **A tier gates binaries and services, never a crate the node links for
> consensus.** `dex`, `rwa`, `identity`, `iot-anchor` and `threat-intel` write
> records under the state root (invariant 25), so gating one behind a cargo
> feature makes two honest nodes compute different state roots. A subsystem is
> turned off with an activation height of `u64::MAX`, which every node compiles
> identically. [ADR-002](docs/adr/ADR-002-feature-tiers.md).

**Class** — `REAL` (reachable and used), `SIM` (a model, never on a production
path, and it says so), `RESEARCH` (in the tree, tested, called by nothing in
consensus).

**Status** — `planned` / `stub` / `working` / `verified`.

Subsystems now: 55 verified, 53 working, 21 planned; 50 core, 57 extended, 22 frontier.

## Roadmap Status

**SHIPPED** — in the tree, reachable, tested. **RESEARCH** — tested, but
*nothing in consensus calls it* (usually activation height `u64::MAX`;
promoting one is a written decision). **PLANNED** — no code; `grep` will not
find it. A dark subsystem and an unwritten one look identical from outside.
Detail and rationale: [docs/architecture-vision.md](docs/architecture-vision.md);
ordering: [docs/trajectory.md](docs/trajectory.md); the thirty `Prompts/`
briefs: [docs/master-prompts/README.md](docs/master-prompts/README.md).

Four PLANNED items collide with recorded invariants and must be reconciled
*before* code: the bytecode hot-patcher (13), relativistic clock sync and
satellite/light-cone consensus (9), TEE attestation (11).

## Critical Invariants

Thirty-two of them, in [docs/invariants.md](docs/invariants.md). They are
numbered, the numbers are cited from code comments and ADRs, and **a number is
never reused**.

Read them before changing: `ledger-math`, `dex`, `governance`, `fee-market` or
`threat-intel` (dependency-freedom for Kani — 1, 6, 12); `crypto-pq` (2, 29);
`fips204` features (4); the suite registry or the hybrid (29, 30); the
transaction wire format or `Transaction::verify` (31); the dev profile overrides (5); DEX or oracle write
paths (7, 8, 9, 17); governance (12, 13); `custody-mpc` (18, 19); zkML
(20–23); `Chain::insert_block` or any apply path (24); any new state prefix
(25); the block store (26); pruning (27); the invariant guard (28); vault
accounts (32).

An invariant earns a number only once a test pins the behaviour. A rule nobody
checks is a comment, and it belongs beside the code it describes.

## Execution Directives

Four standing rules, each stated with its *current* enforcement — a directive
written as achieved is a directive nobody will implement. Expanded in
[docs/architecture-vision.md](docs/architecture-vision.md) §7.

1. **Zeroize cryptographic memory.** `zeroize` is a dependency of every crate
   holding key material; new secret types get `ZeroizeOnDrop`, not a manual
   `drop`, and stay in typed wrappers from generation to use (Standing Order 5).
2. **Deterministic execution, WASM and state.** Enforced mechanically:
   `apply_block_journaled` refuses a `state_root` execution does not reproduce
   (invariant 24); no float enters a consensus rule (invariant 20).
3. **No dynamic heap allocation in critical consensus loops.** A target, not
   today: the apply path allocates (RocksDB owned buffers, the journal's `Vec`).
   Do not add an allocation to an inner loop that lacked one; measure first.
4. **No `unsafe` in core execution paths.** Holds today for `crates/node/src/state/`,
   `crates/node/src/chain.rs`, `ledger-math`, `dex`, `governance`, `fee-market`, `vrf` — the
   same crates whose dependency-freedom exists so Kani can compile them. Exempt
   by construction: `cuda-miner` (CUDA driver FFI), `sdk-ffi` (the C ABI *is*
   the product), `wgpu-miner` (GPU buffer mapping), `hal/ebpf-net/src/linux/`
   (AF_XDP rings and UMEM shared with the kernel; behind `xdp`, so a default
   node links none of it), `hal/ebpf-net/programs` (packet pointers the verifier
   bounds). An `unsafe` block anywhere else needs a hardware-attestation
   justification in a comment, and a reviewer.

## Build & Test

Never run unfiltered cargo output — use `qb` / `qt` / `ql`, or pipe through
`condense`. **Full detail, with every measured figure: [docs/build.md](docs/build.md).**

```powershell
cargo xtask disk                     # artifact size vs the 30 GiB ceiling
cargo xtask coverage --verify-targets # the ledger, and that its tests are real targets
qb                                   # cargo build --workspace
qt                                   # cargo nextest run --workspace
ql                                   # cargo clippy --all-targets
cargo deny check                     # advisories, bans, licences, sources
bash scripts/lint_debt.sh --check    # the clippy::pedantic ratchet
bash scripts/doc_coverage.sh --check # doc coverage, 90% floor
```

Four things that will otherwise cost a day each:

- **Run one cargo scope at a time.** Interleaving `-p` and `--workspace`
  builds, or `cargo xtask` with either, wedges `target/` with
  `required to be available in rlib format`. It is not a profile bug.
- **`jobs = 4` in `.cargo/config.toml` is load-bearing.** Wider builds exhaust
  this machine's commit charge and fail as `os error 1455` while rustc mmaps
  the node rlib — which surfaces as `can't find crate` across the workspace.
- **Disk is the standing hazard.** This volume has been filled to zero bytes
  twice, and a full disk reports as `os error 112`. Run `cargo xtask disk`
  *before* a long build.
- **Profiles are not tuning** — [ADR-003](docs/adr/ADR-003-build-profiles.md).
  The `[profile.dev.package.*]` overrides that name workspace members are the
  only thing optimising them, and invariant 2 depends on one of them.

Measured 2026-09-20: cold `cargo check --workspace --all-targets` 4m 34s,
cold build 4m 54s, `cargo nextest run --workspace` 2,515 tests across 169
binaries all passing, root `target/` 26.4 GiB and 27.7 GiB across every
artifact directory. Measured 2026-09-26 (80 members, 4 vCPU cloud VM):
`cargo test --workspace --profile ci --no-fail-fast` 2,793 passed, 0 failed,
6 ignored across 320 test binaries (doc-tests included) in 41m 0s.

## Code Conventions

- Functions under 50 lines; source files 200–400 typical, 800 soft ceiling;
  nesting depth ≤ 4. Test, generated and vendored files may exceed the ceiling.
- `snake_case` functions and variables, `PascalCase` types,
  `SCREAMING_SNAKE_CASE` constants. Lifetimes short and lowercase.
- Immutable by default; return new values rather than mutating arguments.
- Every error handled explicitly, never swallowed. Typed errors with
  `thiserror` in libraries. Validate at boundaries.
- No magic numbers, no hardcoded secrets, no leftover debug prints.
- **Comments say why, not what.** This repository's manifests and modules
  explain the reasoning behind a boundary, a version pin or a parameter set.
  That prose is the documentation — do not strip it when editing near it.
- `clippy::pedantic` is `warn`, not `deny`, and 1,143 remain at the last ratchet.
  The count is ratcheted: `scripts/lint_debt.sh --check` fails if it rises,
  and `nightly.yml` runs it. It may fall freely. If a change legitimately
  raises it, run `--update` and say why in the commit message.

## Workflow Rules

MCP servers, the ECC harness surface, `.ignore` rules, brand assets and cache
locations are in [docs/environment.md](docs/environment.md) — none of it about
the chain, all of it about the cost of working on it.

1. Navigation order: `skel` / `ctx` → `serena` symbols → `ast-grep` → bounded
   `Read`. Whole-file reads last.
2. Run `/graphify` after major structural changes.
3. Keep files modular — 200-400 lines typical, 800 max.
4. Consensus, crypto, and ledger changes get a `rust-reviewer` +
   `security-reviewer` pass before commit.
5. A new subsystem lands in [docs/architecture-vision.md](docs/architecture-vision.md)
   with a status tag *before* it lands in `Cargo.toml`, gets a `features.toml`
   entry in the same change, and earns a numbered invariant only once a test
   pins the behaviour. Skipping the first step is how a tree acquires a crate
   nobody can explain; skipping the last is how this file acquires a claim
   nobody can check.
6. A new external dependency goes in `[workspace.dependencies]` if any other
   member already uses it — [ADR-005](docs/adr/ADR-005-dependency-unification.md).

## How to Resume

1. `cargo xtask coverage` — the ledger, and whether every claim is still backed.
2. `cargo xtask disk` — artifact size, before starting anything long.
3. `reports/01-foundation.md` — the last measured build and test numbers.
4. [PROGRESS.md](PROGRESS.md) — the first unticked box.
