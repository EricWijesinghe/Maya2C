# 09 — Governance, wallets, explorer and developer ecosystem

**Machine:** cloud VM, 4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4,
`nightly-2026-07-15`. Test lines are from the 2026-09-27 full workspace run.

## Verdict

| DONE WHEN criterion | State | Evidence |
|---|---|---|
| Governance cycle passes | **Met** | `governance_tests.rs` 22 ✓ (propose → vote → timelock → parameter change applied in state) |
| Wallet e2e passes | **Partial** — wallet core passes; the GUI WebDriver e2e was not run (no display/Tauri toolchain on this VM) | §2 |
| Explorer on local devnet | **Partial** — indexer and server tests pass against in-process nodes; not run against a live devnet here | §3 |
| SDK e2e tests pass | **Not met** — no cross-language test that submits a transaction to a local node | §4 |
| `reports/09-product.md` | this file | — |

## 1. Governance

```
tests/governance_tests.rs :: test result: ok. 22 passed; 0 failed; ... in 20.25s
tests/treasury_tests.rs   :: test result: ok. 10 passed; 0 failed; ... in 0.00s
crates/governance tests/limits_tests.rs — passing (governance cannot make governance unsafe, invariants 12, 13, 17)
```

Private ZK voting, quadratic voting, WASM runtime V1 → V2 upgrade with
storage migration, the security-council emergency pause with expiry, and the
AI advisory council are **not built**.

## 2. Wallets

```
apps/wallet-gui/core tests/airgap_tests.rs  :: ok. 5 passed   (animated multi-frame QR export of PQ-signed blobs)
apps/wallet-gui/core tests/hd_tests.rs      :: ok. 21 passed  (mnemonic → deterministic ML-DSA + SLH-DSA keys)
apps/wallet-gui/core tests/payment_tests.rs :: ok. 24 passed
apps/wallet-gui/core tests/vault_tests.rs   :: ok. 17 passed  (encrypted keystore)
```

The air-gapped flow the brief asks for exists: `airgap.rs` frames a signed
transaction as a sequence of QR frames with a protocol (missed frames,
interleaving, repeats). New in this work, the account-model side of Master
Prompt 22 (guardians, spending limits, session keys, vault delays) exists as
`crates/smart-account` (10 tests with real ML-DSA-65).

Not built or not run: passkey unlock; OS keychain integration tests; the
WebDriver e2e (create wallet → submit tx → balance updates) and its FPS
measurement; the "tactical" / calm-accessible theme pair under WCAG checks.

## 3. Explorer, faucet, telemetry, gateway

```
apps/explorer tests/indexer_tests.rs     :: ok. 20 passed
apps/explorer tests/server_tests.rs      :: ok. 23 passed
apps/faucet   tests/http_tests.rs        :: ok. 10 passed
apps/faucet   tests/load_tests.rs        :: ok. 4 passed   (CONCURRENCY = 1_000: the daily cap holds under 1,000 concurrent requests)
crates/telemetry tests/end_to_end_tests.rs :: ok. 6 passed
crates/api-gateway tests/gateway_tests.rs :: ok. 13 passed
crates/api-gateway tests/openapi_tests.rs :: ok. 5 passed  (utoipa OpenAPI 3)
crates/node tests/rpc_tests.rs           :: ok. 13 passed
```

Not built: the 3D Three.js DAG star-chart; Web Audio on finality; the portal's
interactive JSON-RPC playground; dev hub one-click templates and the
"deploy a dApp in under 5 minutes" e2e.

## 4. SDKs

`sdks/sdk-ffi` (uniffi), `sdks/sdk-wasm` (browser, with
`hybrid_parity_tests.rs` 8 ✓ pinning it to the node's hybrid construction),
`sdks/sdk-js`, `sdks/go`, `sdks/python` exist. **No cross-language test
submits a transaction to a local node**, so the SDK e2e criterion is not met.
`.github/workflows/publish_sdk.yml` is dry-run until approved (Standing
Order 6) — unchanged.

## 5. Docs

`docs/site` (Starlight) regenerates its sidebar from `docs/README.md`
(`docs/site/scripts/ingest.mjs`); pages added by this work are listed there.
`cargo doc --no-deps` was not re-run for this report.
