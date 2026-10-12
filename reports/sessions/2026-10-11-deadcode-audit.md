# Dead-code and unnecessary-code audit, 2026-10-11

Read-only. Nothing edited, built or deleted. Findings are candidates for the owner before mainnet (about 2026-10-24).

## Method and commands

- cargo xtask disk: artifacts 117.3 GiB vs 30.0 GiB ceiling (warning only; no build run).
- cargo metadata --no-deps --format-version 1: 96 workspace members (CLAUDE.md says 87).
- Dependents: python -I over metadata, normal (non-dev) deps between workspace members.
- rg for each candidate crate use-paths and manifest refs, excluding target/ and .claude/.
- rg for each candidate path in features.toml, docs/architecture-vision.md, .github, xtask, scripts.
- cargo machete (no build). Output saved to scratchpad machete.txt.
- scratchpad/audit.py (python -I) modes: pub, dupes, comment, stubs, todo.
  - pub: pub identifiers occurring once across all .rs files (definition only), node-linked crates.
  - dupes: fn bodies of 8+ code lines, whitespace-normalised hash, across more than one file, tests and benches excluded.
  - comment: runs of 10+ commented-out code lines.
  - stubs: files under 6 code lines, or todo!/unimplemented!.
- rg for allow(dead_code|unused), todo!/unimplemented!, quarantin, *.bak, *_old*, *.orig, *.old.
- features.toml parsed with tomllib: 313 entries (164 feature, 149 subsystem), 194 RESEARCH or SIM.

NOT VERIFIED:
- cargo udeps: not installed. Machete is a text heuristic; macro and cfg-only uses can false-positive.
- Serena find_referencing_symbols: not run. The pub scan is a word-count heuristic and under-reports.
- Compile check of any removal: not run (read-only).
- cargo xtask coverage --verify-targets: not run.

## Workspace-level dependents

| Crate / bin | Normal dependents | features.toml | Note |
|---|---|---|---|
| maya-tiers (crates/tiers) | none | no entry | Intentional facade (see Do NOT remove) |
| maya-timing | none | timing-service RESEARCH working | 8 inline tests; keep-dark |
| maya-deposit-watcher | none (own test only) | exchange-deposit-watcher REAL verified | Class contradicts CLAUDE.md (REAL = reachable and used) |
| maya-da (crates/da) | none (own tests, benches) | dag-bft tests reference it | Investigate whether node has a parallel DA path |
| maya-entropy | none (lib); bin ships | entropy-hal REAL verified | Bin is the reachable artifact; keep |
| maya-wallet-e2e | none | no entry | E2E harness; investigate |
| maya-mesh-api, maya-status, maya2c-cli, maya2c-mcp | bins | REAL working | Keep |
| btc-bridge, bci-sim, eo-oracle, energy, interop, lanes, multivm, parallel-exec, permissioned-finance, reference-apps, state-sync, dos-guard, history-compactor, swarm, link-sim, econ | none outside own tests | RESEARCH or SIM | Keep-dark, features-tracked |

No crate is safe to delete on dependency grounds alone.

## Ranked removal and merge candidates

| # | Path | What | Evidence | Risk | Recommendation |
|---|---|---|---|---|---|
| 1 | .claude/worktrees/ (untracked, 62 MB) | session worktree copies | du, git status | none to build | delete only after confirming no session is using it |
| 2 | 18 unused deps in 12 manifests (list below) | cargo-machete | machete.txt | low; macro/cfg false positives possible | delete per crate, cargo check one scope at a time |
| 3 | bins/attacknet-controller/src/validators, scenarios, metrics, evidence, config .rs | 17 allow(dead_code), no justification comment | rg | low, lab tool | delete the methods or gate with cfg(test) |
| 4 | crates/node/src/state/dex.rs:232,254,293 | balance_prefix, side_prefix, order_index_len: zero callers | pub scan | low; written keys unchanged | delete after cargo check |
| 5 | crates/node/src/state/dex_exec.rs:98 | MAX_ORDERS_PER_BOOK = 4_096, zero code refs. Real cap is ParameterKey::DexMaxOrdersPerBook (dex_exec.rs:775); default 4096 repeated at crates/governance/src/params.rs:195 | rg | moderate: a governance default change leaves the constant stale | delete, or derive from bounds().default; fix docs/dex.md:227,237 |
| 6 | hal/cuda-miner/src/dag.rs:124,137 vs crates/node/src/crypto/dag/params.rs (largest_prime_at_most, is_prime) | duplicate DAG-size helpers, identical bodies. cuda-miner already depends on custom-l1-node | dupes | moderate: GPU miner sizes its DAG from its copy; drift silently changes the mining dataset | make node helper pub, call from cuda-miner, delete copy |
| 7 | crates/node/src/state/dex.rs:296, governance/mod.rs:92, sealed/mod.rs:146 | join() state-key builder, same body 3x | dupes | moderate: state-key divergence would be silent | merge to one helper |
| 8 | pub zero-caller fns in node (below) | unwired or dead | pub scan | varies | investigate consensus-named ones first |
| 9 | apps/explorer vs bins/pool-service: format_hashrate, into_response, html | duplicate UI helpers | dupes | cosmetic | merge into shared helper |
| 10 | xtask/src/attacknet.rs:187 vs localnet.rs:235 wait_until; crypto-pq hex_prefix (hqc.rs:312, kem.rs:262) | duplicates | dupes | cosmetic | merge |
| 11 | 8 scripts referenced only from docs: attacknet-daily.ps1, bft_devnet.py, build-contract.sh, check_brand_refs.py, make_zkml_fixture.py, pre_ignition_audit.sh, reality_audit.py, runbook_rehearsals_linux.sh | operator tools | basename rg | low | investigate runbook usage |

Pub zero-caller items in node-linked crates (identifier appears only at its definition):

- Investigate, may be an unwired check: consensus/chain.rs:718 expected_window_timespan, core/block.rs:201 meets_difficulty_at, config.rs:256 dual_kem_policy, consensus/bft/driver.rs:508 is_follower, state/council.rs:88 committed_council, network/mempool.rs:349 revalidate, network/radio_gateway.rs:214 finish_object.
- Delete candidates: crypto/hybrid.rs:372 from_halves, crypto/keys.rs:371 verifying_key_from_bytes, core/sealed_payload.rs:43 SHARE_SIZE, state/asset.rs:211 native_balance, state/channel.rs:123 counterparty_of, state/governance_exec.rs:570 proposal_state, state/settlement.rs:607 account_for, tests/common/mod.rs:63,79.

Unused deps (machete, excluding maya-tiers):

- apps/dashboard: serde. apps/explorer: chrono. apps/wallet-gui/src-tauri: serde_json, zeroize.
- bins/l1-wallet: serde, serde_json. bins/pool-service: tower-http.
- crates/archive, crates/mesh-api, crates/vm: hex. crates/telemetry: futures-util. crates/zk-stark: p3-poseidon2, serde.
- hal/wgpu-miner: clap, crossbeam-channel, hex, tokio.
- sdks/sdk-ffi: maya-crypto-pq (investigate; may be needed for uniffi linkage).

## Leftovers

- TODO/FIXME/XXX in Rust: 0 (6 total, all in generated Swift/Kotlin bindings or docs).
- *.bak, *_old*, *.orig, *.old: none.
- Commented-out code runs of 10+ lines: none.
- Stub-sized files: crates/treasury, crates/interop, crates/reference-apps, crates/signer, crates/node/src/neural_gas/mod.rs, crates/node/src/state/invariant_guard/limits.rs, plus main.rs and build.rs stubs. Re-export or constant modules, not dead code.
- allow(dead_code): 19 sites. 17 in attacknet-controller with no justification. crates/node/benches/crypto.rs:50 has a comment pointing to the test file. crates/api-gateway/src/jsonrpc.rs:45 is justified (OpenAPI wire shape).

## Do NOT remove

- crates/tiers (maya-tiers): facade by design, no code. The 22 machete hits are intentional (ADR-002).
- crates/spec-ref (maya-spec-ref): generator for spec/tests/*.json conformance vectors. Load-bearing.
- bins/genesis-ceremony, bins/maya2c-genesis: used by scripts/local_cluster.sh and docs/site/authored/mining.md.
- bins/maya2c-signer (remote-validator-signer REAL working); bins/maya2c-miner, bins/maya2c-gpu-miner (gpu-mining REAL verified).
- bins/attacknet-controller: live gate-10 lab tooling (ATTACKNET_LAB_GUIDE.md). The allows are removable; the bin is not.
- crates/node/src/crypto/keys.rs:297,301 unimplemented!() in SeedRng: fips204 RngCore contract. Non-test panic path; keep and note.
- RESEARCH core-tier dark: dos-guard, parallel-exec, state-sync, history-compactor. Features-tracked. dos-guard may be a missing control; confirm wiring before mainnet.
- ParameterKey::DexMaxOrdersPerBook path: live. Only the duplicate constant is at issue.

## Doc and claim drift

- CLAUDE.md "87 members": actual 96.
- features.toml REAL verified for exchange-deposit-watcher and entropy-hal, with no non-test lib dependents. CLAUDE.md defines REAL as reachable and used.
- docs/dex.md:227 names MAX_ORDERS_PER_BOOK as the cap. Enforcement is the governance parameter.

## Priority

- Now: items 1 (after check), 2 (per crate), 3, 4.
- Before mainnet, with a test: items 5, 6, 7.
- Investigate, do not delete: consensus-named zero-callers, dos-guard wiring, maya-da, maya-deposit-watcher class.
- Batch later: items 9, 10.
