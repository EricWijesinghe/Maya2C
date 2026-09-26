# 11 — Reality audit of Master Prompts 1–10

**Machine:** cloud VM (a fresh container: no `target/`, no registry cache),
4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4, kernel 6.18.44,
`rustc 1.99.0-nightly (da80ed070 2026-07-14)`. The previous measurements in
`reports/01-foundation.md` were taken on Windows 11 with 24 CPUs; the figures
below are this machine's and are not comparable one-for-one.

## 1. From a clean start

The container started with no build artifacts, which is stronger than
`cargo clean` (no registry cache either: `cargo fetch` downloaded 1.3 GB).

| Check (MP1 DONE WHEN) | Result |
|---|---|
| `cargo check --workspace --all-targets` | **passes, 0 warnings**, cold, 17 min 26 s (user 52 min 38 s) |
| `cargo test --workspace --profile ci --no-fail-fast` | **234 test binaries: 2,626 passed, 0 failed, 6 ignored**, 55 min 50 s — raw log `reports/raw-test-baseline-2026-09-27.log` |
| `features.toml` lists 164 register entries | yes; plus 110 subsystems after this work |
| `cargo xtask coverage` | `features.toml: 164 register entries + 110 subsystems, all claims backed.` |
| clippy clean for core | see §5 (`scripts/lint_debt.sh`) |
| target/ under 30 GiB after a full build | `target/` 8.5 GiB after check + full test build on this machine (`du -sh target`) |
| CLAUDE.md, PROGRESS.md, ADR-001 exist | yes |

The one warning line in the test run is the known future-incompatibility
notice for `proc-macro-error2 v2.0.1` (transitive; PROGRESS.md lists it).

## 2. Status re-derived from evidence

`python3 scripts/reality_audit.py reports/raw-test-baseline-2026-09-27.log`:
an entry keeps `working`/`verified` only if every test it names *ran* in that
log with `0 failed`. **140 claims examined; 131 held.** The 9 that the script
would move are not failures:

- 8 are subsystems added by this work after that run started
  (`btc-spv`, `econ-sim`, `link-sim`, `timing-service`, `loadgen`,
  `parallel-exec`, `smart-account`, `data-availability`); each passed in its
  own run (the per-crate reports), and the final full run re-checks them.
- `ledger-hardware-wallet-app`: its tests live in `apps/ledger-maya2c/`, a
  separate workspace built for the device target with Speculos; they were not
  run on this machine. **Unverified in this audit**, status left as recorded
  with this note, because "not run here" is not "failed".

Five entries are backed by a command rather than a test (`NEEDS-GATE`:
supply-chain gates, benchmark suite, memory, xtask); a test log cannot speak
for them. Full table:

| id | before | after | evidence |
|---|---|---|---|
| `ml-kem-768-fips-203` | verified | verified | all named tests ran and passed |
| `ml-dsa-65-fips-204` | verified | verified | all named tests ran and passed |
| `slh-dsa-fips-205` | verified | verified | all named tests ran and passed |
| `hqc-hamming-quasi-cyclic` | verified | verified | all named tests ran and passed |
| `shielded-pool-stark-joinsplit` | verified | verified | all named tests ran and passed |
| `mpc-tss-threshold-custody-m-of-n` | verified | verified | all named tests ran and passed |
| `m-of-n-ml-dsa-multisig` | verified | verified | all named tests ran and passed |
| `threshold-lattice-signing` | working | working | all named tests ran and passed |
| `verifiable-random-function-rfc-9381` | verified | verified | all named tests ran and passed |
| `threshold-encrypted-mempool` | verified | verified | all named tests ran and passed |
| `self-sovereign-identity-records` | working | working | all named tests ran and passed |
| `did-key-rotation-and-revocation` | working | working | all named tests ran and passed |
| `selective-disclosure-credentials` | working | working | all named tests ran and passed |
| `hash-lock-htlc-atomic-swaps` | verified | verified | all named tests ran and passed |
| `lattice-htlc-l-atomic-swaps` | working | working | all named tests ran and passed |
| `hybrid-signature-transaction-validation` | verified | verified | all named tests ran and passed |
| `ethash-style-dag-proof-of-work` | verified | verified | all named tests ran and passed |
| `committed-transaction-root-state-root` | verified | verified | all named tests ran and passed |
| `undo-journal-and-reorg-safety` | verified | verified | all named tests ran and passed |
| `wasm-runtime-via-cranelift-jit` | verified | verified | all named tests ran and passed |
| `historical-pruning-archive-bootstrap` | verified | verified | all named tests ran and passed |
| `constant-product-dex-order-book` | verified | verified | all named tests ran and passed |
| `governance-lifecycle-and-bounds` | verified | verified | all named tests ran and passed |
| `eip-1559-base-fee-over-bytes` | working | working | all named tests ran and passed |
| `neural-base-fee-gain` | working | working | all named tests ran and passed |
| `multi-shard-asynchronous-dag-narwhal-tusk` | working | working | all named tests ran and passed |
| `elastic-shard-auto-scaling` | working | working | all named tests ran and passed |
| `lattice-proof-of-useful-work-svp-solver` | working | working | all named tests ran and passed |
| `stateless-transfer-verification` | working | working | all named tests ran and passed |
| `libp2p-transport-pq-noise-handshake` | verified | verified | all named tests ran and passed |
| `byzantine-peer-guard` | verified | verified | all named tests ran and passed |
| `stratum-v2-pool-protocol` | verified | verified | all named tests ran and passed |
| `spv-light-client` | verified | verified | all named tests ran and passed |
| `network-simulation-harness` | verified | verified | all named tests ran and passed |
| `ebpf-xdp-relay-accelerator-aya` | working | working | all named tests ran and passed |
| `lora-off-grid-transport` | working | working | all named tests ran and passed |
| `fountain-coded-fragmentation` | working | working | all named tests ran and passed |
| `store-and-forward-mesh-relay` | working | working | all named tests ran and passed |
| `gpu-mining-cuda` | verified | verified | all named tests ran and passed |
| `gpu-mining-wgpu-vulkan-metal-dx12` | verified | verified | all named tests ran and passed |
| `confidential-federated-training` | working | working | all named tests ran and passed |
| `iot-anchor-puf-tpm-sealed-device-identity-was-tp` | working | working | all named tests ran and passed |
| `fuzz-targets-cargo-fuzz-libfuzzer` | verified | verified | all named tests ran and passed |
| `kani-model-checking` | verified | verified | all named tests ran and passed |
| `supply-chain-gates` | verified | verified | NEEDS-GATE: cargo deny check && cargo audit |
| `telemetry-threat-surface` | verified | verified | all named tests ran and passed |
| `state-invariant-guard-circuit-breaker` | verified | verified | all named tests ran and passed |
| `exploit-replay-suite` | verified | verified | all named tests ran and passed |
| `libafl-dynamic-fuzzer` | working | working | all named tests ran and passed |
| `threat-intel-registry-was-zk-siem-threat-mesh` | working | working | all named tests ran and passed |
| `iso-20022-xml-messaging-parser` | working | working | all named tests ran and passed |
| `iso-20022-l1-bridge` | working | working | all named tests ran and passed |
| `sanctions-non-membership-proofs` | working | working | all named tests ran and passed |
| `real-world-asset-primitives` | working | working | all named tests ran and passed |
| `atomic-delivery-versus-payment` | working | working | all named tests ran and passed |
| `jurisdictional-transfer-rules` | working | working | all named tests ran and passed |
| `pro-rata-revenue-distribution` | working | working | all named tests ran and passed |
| `tauri-2-0-desktop-wallet` | verified | verified | all named tests ran and passed |
| `leptos-block-explorer-and-dashboard` | verified | verified | all named tests ran and passed |
| `axum-rest-graphql-gateway` | verified | verified | all named tests ran and passed |
| `multi-language-sdks` | verified | verified | all named tests ran and passed |
| `latex-technical-reference-generator` | verified | verified | all named tests ran and passed |
| `testnet-faucet` | verified | verified | all named tests ran and passed |
| `l2-flash-settlement` | verified | verified | all named tests ran and passed |
| `command-line-wallet` | verified | verified | all named tests ran and passed |
| `wallet-core-gui-shared` | verified | verified | all named tests ran and passed |
| `ledger-hardware-wallet-app` | working | **stub** | not in this run: apps/ledger-maya2c/tests/apdu_tests.rs, apps/ledger-maya2c/tests/lowmem_tests.rs, apps/ledger-maya2c/tests/parity_tests.rs |
| `mining-pool-service` | verified | verified | all named tests ran and passed |
| `genesis-ceremony` | verified | verified | all named tests ran and passed |
| `benchmark-suite` | verified | verified | NEEDS-GATE: cargo bench --workspace --no-run |
| `project-memory-and-session-handoff` | verified | verified | NEEDS-GATE: cargo xtask coverage |
| `deterministic-simulation-harness` | verified | verified | all named tests ran and passed |
| `workspace-automation-xtask` | verified | verified | NEEDS-GATE: cargo xtask coverage && cargo xtask disk |
| `signature-suite-registry` | verified | verified | all named tests ran and passed |
| `suite-envelope-crypto-agility` | verified | verified | all named tests ran and passed |
| `kem-suites-dualkem-xwing` | verified | verified | all named tests ran and passed |
| `entropy-hal` | verified | verified | all named tests ran and passed |
| `transparent-stark-plonky3` | working | working | all named tests ran and passed |
| `forward-secure-archival-keys` | working | working | all named tests ran and passed |
| `history-compactor` | working | working | all named tests ran and passed |
| `dag-bft` | working | working | all named tests ran and passed |
| `vm-execution-tiers` | working | working | all named tests ran and passed |
| `btc-spv` | working | **stub** | not in this run: crate:maya-btc-spv |
| `econ-sim` | working | **stub** | not in this run: crate:maya-econ, econ/tests/scenario_tests.rs |
| `link-sim` | working | **stub** | not in this run: crate:maya-link-sim |
| `timing-service` | working | **stub** | not in this run: crate:maya-timing |
| `loadgen` | working | **stub** | not in this run: crate:maya-loadgen |
| `parallel-exec` | working | **stub** | not in this run: crates/parallel-exec/tests/differential.rs |
| `smart-account` | working | **stub** | not in this run: crates/smart-account/tests/account_tests.rs |
| `data-availability` | working | **stub** | not in this run: crates/da/tests/da_tests.rs |
| `P001` | verified | verified | all named tests ran and passed |
| `P002` | verified | verified | all named tests ran and passed |
| `P003` | verified | verified | all named tests ran and passed |
| `P004` | verified | verified | all named tests ran and passed |
| `P005` | verified | verified | all named tests ran and passed |
| `P006` | verified | verified | all named tests ran and passed |
| `P007` | verified | verified | all named tests ran and passed |
| `P008` | verified | verified | all named tests ran and passed |
| `P009` | verified | verified | all named tests ran and passed |
| `P010` | verified | verified | all named tests ran and passed |
| `P011` | verified | verified | all named tests ran and passed |
| `P012` | verified | verified | all named tests ran and passed |
| `P013` | verified | verified | all named tests ran and passed |
| `P014` | verified | verified | all named tests ran and passed |
| `P015` | verified | verified | all named tests ran and passed |
| `P016` | verified | verified | all named tests ran and passed |
| `P017` | verified | verified | all named tests ran and passed |
| `P018` | verified | verified | all named tests ran and passed |
| `P019` | verified | verified | all named tests ran and passed |
| `P020` | verified | verified | all named tests ran and passed |
| `P021` | verified | verified | all named tests ran and passed |
| `P022` | verified | verified | all named tests ran and passed |
| `P023` | verified | verified | all named tests ran and passed |
| `P024` | verified | verified | all named tests ran and passed |
| `P025` | verified | verified | all named tests ran and passed |
| `P026` | verified | verified | all named tests ran and passed |
| `P027` | verified | verified | all named tests ran and passed |
| `P028` | verified | verified | all named tests ran and passed |
| `P029` | verified | verified | all named tests ran and passed |
| `P030` | verified | verified | all named tests ran and passed |
| `P031` | verified | verified | all named tests ran and passed |
| `P032` | verified | verified | all named tests ran and passed |
| `P033` | verified | verified | all named tests ran and passed |
| `P034` | verified | verified | all named tests ran and passed |
| `P035` | verified | verified | all named tests ran and passed |
| `P036` | verified | verified | all named tests ran and passed |
| `P037` | verified | verified | all named tests ran and passed |
| `P038` | verified | verified | all named tests ran and passed |
| `P039` | verified | verified | all named tests ran and passed |
| `P040` | verified | verified | all named tests ran and passed |
| `P041` | verified | verified | all named tests ran and passed |
| `P042` | verified | verified | all named tests ran and passed |
| `P043` | verified | verified | all named tests ran and passed |
| `P044` | verified | verified | all named tests ran and passed |
| `P045` | verified | verified | all named tests ran and passed |
| `P046` | verified | verified | all named tests ran and passed |
| `P047` | verified | verified | all named tests ran and passed |
| `P048` | verified | verified | all named tests ran and passed |
| `benchmark` | verified | verified | NEEDS-GATE: cargo bench --workspace --no-run |
| `memory` | verified | verified | NEEDS-GATE: cargo xtask coverage --verify-targets |

140 claims examined, 9 would change status on this log's evidence.


## 3. Master Prompts 1–10, DONE WHEN re-checked

| MP | DONE WHEN | Re-checked here |
|---|---|---|
| 01 | check/clippy/target/features/CLAUDE/PROGRESS/ADR-001/report | met (§1); clippy count §5 |
| 02 | REAL items pass NIST vectors; labels; ADRs; report | `acvp_tests.rs` 6 ✓, `kem_kat_tests.rs` 3 ✓, `drbg_cavp_tests.rs` 3 ✓ in the full run; ADR-007…014 present |
| 03 | state + property tests, fee market active, pruned node, DNA, report | fee market **not active**; the rest met — `reports/03-state.md` |
| 04 | three modes in sim, reorg + DAG commit, GPU parity or skip, TPS report | met at engine level; TPS not measurable — `reports/04-consensus.md` |
| 05 | VM + differential gas, multi-VM A, zkML verify time, report | VM + differential met; multi-VM and zkML **not built** — `reports/05-vm.md` |
| 06 | every module tested + ledger + invariant hooks, report | partial — `reports/06-finance.md` |
| 07 | P2P/PQ tests, eBPF load or reason, transport sims, report | met with eBPF skipped for a stated reason — `reports/07-network.md` |
| 08 | security report with raw outputs; invariants proven or open | `reports/08-security.md` |
| 09 | governance, wallet e2e, explorer, SDK e2e, report | governance met; wallet/explorer partial; SDK e2e **not met** — `reports/09-product.md` |
| 10 | deploy dry-run on k3d with 12 nodes; report; LAUNCH.md | `reports/10-launch.md`, `LAUNCH.md` |

## 4. Release check — both runs (Master Prompt 11 §3)

**Clean run** (`cargo xtask release-check`):

```
  [ok]   symbol table: no symbol from any SIM crate
  [info] RESEARCH crates linked dark for consensus determinism (ADR-002): maya-identity, maya-htlc-lattice, maya-blockgraph, maya-lattice-pow, maya-stateless-core, maya-ebpf-net, maya-radio-transport, maya-iot-anchor, maya-threat-intel, maya-iso20022, maya-rwa
release-check: PASS

real	2m55.729s
user	10m40.573s
sys	0m25.672s
exit=0
```

**Forced SIM feature** (`cargo xtask release-check --force-sim`), which must
fail:

```
release-check --force-sim: production build with maya-entropy/sim-sources forced in
  error: a `production` build has a SIM, RESEARCH, frontier, test-util or insecure feature enabled somewhere in its dependency graph; run `cargo tree -e features -i maya-build-guard` to find which crate forwarded it (ADR-016)
release-check --force-sim: FAIL, as expected: the build guard refused the build
xtask: expected failure: production + sim-sources refused by the build guard
exit=1
```

**What the guard found on its first run:** the clean production build failed.
`cargo tree -e features -i maya-build-guard` showed `custom-l1-node` enabling
`maya-crypto-pq/hqc` unconditionally — every node binary linked the draft HQC
KEM (non-constant-time decapsulation, ADR-009). HQC is now the node's
default-on `hqc` feature; production builds use `--no-default-features`,
omit it, and refuse `dual_kem = "preferred" | "required"` at config load.

**The production binary refuses to start**, by design (ADR-016):

```
$ ./target/ci/maya2c-node --genesis /dev/null
Error: "production build: consensus mode `argonblake-pow` is devnet-only; mainnet runs dag-bft, which is not yet wired into this node (ADR-015, ADR-016)"
```

## 5. Gap register

`reports/11-gap-register.md` (generated by `scripts/gap_register.py`):
27 items — 3 P0, 0 P1, 24 P2. The tree has **no** `TODO`/`FIXME` comments;
the P0 items are two `unimplemented!()` in an RNG adapter that ML-DSA key
generation never calls (`crates/node/src/crypto/keys.rs:297,301`) and one
`panic!` in a `const fn` that can only fire at compile time
(`crates/crypto-pq/src/kem_suite/dual.rs:59`). All three are unreachable at
runtime; they are listed because the register lists, it does not judge.
