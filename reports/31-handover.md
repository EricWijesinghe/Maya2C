# Handover — 2026-09-28

The build the owner receives, the evidence behind it, and the steps only the
owner can take. Every figure here was measured on the Windows 11 workstation
(Intel family 6 model 198, `nightly-2026-07-15`) at commit `bff254e` on
`feat/complete-session-work`, unless a line says otherwise.

**Verdict:** the build is deployable to a **private or public test network**
today. It is **not** ready for a value-bearing mainnet, and
`cargo xtask go-no-go` says so from evidence: its failing gates need time,
money or other people (an external audit, a four-week attacknet, an
incentivized testnet, external validators, a signed genesis), not more code.

## 1. The artifacts

`dist/maya2c-0.1.0-x86_64-pc-windows-msvc/` (ignored by git; rebuild with the
commands below). The `dist` profile: `opt-level = 3`, fat LTO, one codegen
unit, stripped, `panic = "abort"`.

```
cargo build --profile dist -p maya2c-node --no-default-features --features production
cargo build --profile dist -p l1-wallet -p maya2c-cli -p maya-api-gateway -p maya2c-genesis \
  -p genesis-ceremony -p maya2c-peerid -p maya-mesh-api -p maya2c-signer -p maya-htlc-watcher -p maya2c-miner
```

| Binary | Bytes | What it is |
|---|---|---|
| `maya2c-node` | 24,726,528 | the node, `--features production`: DAG-BFT when the genesis names a committee, PoW otherwise |
| `l1-wallet` | 3,281,920 | CLI wallet: keystore, send, vault |
| `maya2c` | 22,146,560 | developer CLI: dev chain, debugger (CLI and DAP), replay, fork, vault, custody report |
| `maya2c-gateway` | 6,075,392 | REST gateway for the SDKs |
| `maya2c-mesh` | 4,780,032 | Mesh (Rosetta) Data API for exchanges |
| `maya2c-genesis` | 520,704 | genesis file builder |
| `genesis-ceremony` | 616,960 | multi-party genesis signing |
| `maya2c-peerid` | 294,400 | node identity / PeerId for bootnode addresses |
| `maya2c-signer` | 589,824 | remote validator signer |
| `htlc-watcher` | 2,984,448 | cross-chain hash-locked swap agent |
| `maya2c-miner` | 17,669,120 | PoW miner (PoW networks only) |

Every binary answers `--version`. `SHA256SUMS` sits beside them. The node
took 610 s to build cold with fat LTO; the other ten together took less.

**Not built for other platforms here.** Linux binaries come from the same
commands on Linux (CI builds Linux; the developer path was measured in WSL,
`reports/17-integrations.md`). There is no macOS build: no Mac was
available.

## 2. Evidence

| Check | Result |
|---|---|
| `cargo xtask release-check` | **PASS**: production build compiles; no forbidden feature or SIM crate in `cargo tree`; **no SIM symbol in the binary** (read from its PDB — previously skipped on Windows) |
| `MAYA2C_BIN_DIR=target/dist cargo xtask localnet` | **PASS** in 14.8 s on the shipped binaries: 4 validators as processes over libp2p agree on one chain; a transfer executes on all 4 in 2.0 s; with one killed the other 3 commit 5 more blocks; restarted, it catches up in 0.52 s |
| `cargo xtask spec-coverage` | 44 rules, 71 vectors, **0 consensus gaps**; the node agrees on every vector |
| `cargo xtask coverage --verify-targets` | 164 register entries + 143 subsystems, every claim backed by a test target |
| `bash scripts/lint_debt.sh --check` | 1,140 pedantic diagnostics, at the baseline |
| `cargo deny check` | advisories, bans, licences, sources **ok** (after `ruint` 1.20.1 for RUSTSEC-2026-0220) |
| `cargo xtask readiness` | 47 of 104 cells link evidence on disk; the rest are GAP by name |
| `cargo xtask go-no-go` | **8 PASS, 5 FAIL, 3 NEEDS HUMAN — NO-GO**; every FAIL is an owner item in §4 |
| full workspace tests | **2,933 passed, 0 failed**, 10 skipped (§3) |

## 3. Test run

```
$ cargo nextest run --workspace --no-fail-fast
     Summary [ 227.157s] 2933 tests run: 2933 passed (11 slow, 8 leaky), 10 skipped
```

274 s wall clock, warm. It took three full runs to get here. The first
failed 6 of 2,933, from three causes, all fixed and committed (`bff254e`):

- **deposit-watcher** could not discard a torn journal line on Windows. An
  append-only handle may not truncate. A real bug, not a test artefact: a
  watcher restarted after a crash mid-write would have refused to start.
- **Four devnet tests** built `maya2c-node` inside the run, then ran it from
  a shared path. Windows would not replace a running executable. Each test
  now runs a private copy built in `target/test-bins`.
- **The kill -9 test** timed its kill from spawn. Under load, most kills hit
  startup, not commit. It now times from the child's "committing" line:
  1,000 of 1,000 runs committed and every restart was consistent. Per change
  it runs 250; nightly runs 1,000.

"Leaky" is nextest's word for a test that left a handle or child process
open past its end; none failed. The 10 skipped are `#[ignore]` tests that
need an hour, hardware or a release build, each with its reason in the
attribute.

`SHA256SUMS`:

```
14ca0fc223b5453329f129ae6ef1c1b2351d78911099f5a7a5bdfd32a71b8f44 *genesis-ceremony.exe
a6397b11e51cca72a522fb7c08351e42ba439ff82f928c5ab72ec5eb7ee18b2e *htlc-watcher.exe
488edb9c6949e575a158da561099a4caaf27ed4e8695d44c3a182f0220c81dfc *l1-wallet.exe
4f33def60769441552eed17e6f180d0868b1c7cc78423c65492433e8da5f852a *maya2c-gateway.exe
eb1c0473f13baf4e135023982c500205360714540f34bc1bfa4a6c64a383e2f5 *maya2c-genesis.exe
320d64f9c5e539c75787dfc606e5c38b4bd0c0509c878f9360e89fe60b4519a9 *maya2c-mesh.exe
17a18f40dca3c5067a63ce7d141396028770d9042b91e0af979c98e3a4f7dc34 *maya2c-miner.exe
3f0022817391a69455add3ac03ba6f4b58aa97b15cbae7ebebae449ccc7347c8 *maya2c-node.exe
254130850f2df7b917efe0f8bb1d57082fddbd660a513ffc1747d45038d2c95b *maya2c-peerid.exe
a50bee203d62c1a0c0448d783ccc9a2a564a800721634f87c8727def99c9d8d1 *maya2c-signer.exe
d9a853603d18d4a7bd21fa84d63a197d0a099d007ae44f64389d9c1d9fdd5248 *maya2c.exe
```

These are local builds on a developer workstation, not reproducible-build
artifacts, and they are not code-signed. A release for others needs both:
a signing certificate is an owner purchase.

## 4. What the owner must do

These cannot be done by code in this repository. Each is recorded where it
is enforced.

1. **External security audit** of consensus, crypto-pq, the VM host
   functions, custody and the privacy circuits — go-no-go gate "External
   audits". Budget and vendor are the owner's decision.
2. **Attacknet for four weeks** and an **incentivized testnet meeting SLOs
   for four weeks** — go-no-go gates; they need a public network and time.
3. **External validators** across providers and countries — go-no-go gate.
4. **Genesis ceremony**: freeze parameters and sign
   `genesis/mainnet.json.sig` with `genesis-ceremony` — go-no-go gate.
5. **Legal sign-off** and a **staffed on-call rota**
   (`docs/security/INCIDENT_RESPONSE.md`) — go-no-go NEEDS HUMAN.
6. **Put `bft.fees` in the mainnet genesis**. The fee market charges from
   block 1 only when the genesis carries it — go-no-go NEEDS HUMAN.
7. **Spend and deploy approvals**: `terraform apply`, `deploy-production.sh`,
   any package publish, each needs `APPROVED: <step>` (Production Standing
   Orders). Nothing here has touched a live server or spent money.
8. **A dedicated benchmark runner** (`BENCH_RUNNER`) so the >5% regression
   gate runs (MP12).
9. **WalletConnect Cloud project id** and **a Mac** (MP17); **a phone** for
   mobile proving (MP27); **participants** for the usability studies (MP22,
   MP29).
10. **Crypto-agility crates publishing**: licence choice and
    `APPROVED: publish` (MP28).
11. **A code-signing certificate** for the Windows binaries (and an Apple
    developer account for macOS), so users are not warned off the download.

## 5. Built but dark

RESEARCH subsystems are in the tree and tested, and nothing in consensus
calls them until someone writes an activation height. The node links twelve
of them dark (listed by `release-check`). The shielded pool and private
contract state wait on a circuit audit (ADR-016).

## 6. Not built

From the prompt ledger (`docs/master-prompts/README.md`), what remains after
this session, each with its reason:

- Lock/mint bridge for outside assets; ZK light client (MP06, MP25).
- EEG/BCI SIM, satellite NDVI oracle, swarm auctions, 10,000-device barter
  (MP06).
- Malicious-secure dark-pool MPC (share MACs, per-order proofs) (MP06).
- Liveness and uniqueness for proof-of-personhood (MP06).
- Go and Python SDKs are placeholders (MP09).
- Wallet GUI WebDriver suite: never run, and **cannot pass as written**. 10 of its 12 element ids are not in the UI, and two of its three listed specs do not exist (`apps/wallet-gui/e2e/README.md`) (MP09, MP29).
- Source-line mapping and lazy fork state for the debugger (MP24).
- A public testnet and outside developers (MP30).
