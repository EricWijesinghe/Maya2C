# 09 — Governance, wallets, explorer and developer ecosystem

**Machine:** cloud VM, 4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4,
`nightly-2026-07-15`. Test lines are from the 2026-09-27 full workspace run.

## Verdict

| DONE WHEN criterion | State | Evidence |
|---|---|---|
| Governance cycle passes | **Met** | `governance_tests.rs` 22 ✓ (propose → vote → timelock → parameter change applied in state) |
| Wallet e2e passes | **Partial** — wallet core passes; the GUI WebDriver e2e was not run (no display/Tauri toolchain on this VM) | §2 |
| Explorer on local devnet | **Partial** — indexer and server tests pass against in-process nodes; not run against a live devnet here | §3 |
| SDK e2e tests pass | **Met for TypeScript** (2026-09-27) — a Rust-signed transfer submitted by `maya2c.js` through `maya2c-gateway` to a live DAG-BFT node, finalized in ~4 s; found and fixed two gateway bugs. Go and Python SDKs do not exist | §4 |
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

`sdks/sdk-ffi` (uniffi), `sdks/sdk-wasm` (browser, verify-only, with
`hybrid_parity_tests.rs` 8 ✓ pinning it to the node's hybrid construction)
and `sdks/sdk-js` exist. **`sdks/go` and `sdks/python` are README-only
placeholders** — an earlier version of this section said they existed.

### SDK end to end (2026-09-27, Windows workstation)

`python scripts/sdk_e2e.py` starts a one-validator DAG-BFT devnet, starts
`maya2c-gateway` in front of it, signs a transfer with
`l1-wallet send --no-broadcast` (Rust), and runs
`sdks/sdk-js/test/live-node.test.ts` against the gateway (TypeScript):

```
signed in Rust: txid add798254a5d39d6160c98755f411a5b6690401151829030dbb6cafa8c08eb4b
sdk e2e: submitted via gateway, recipient credited after 3995 ms
 ✓ test/live-node.test.ts (2 tests) 8066ms
      Tests  2 passed (2)
```

It checks the recipient is credited by exactly the amount, the sender's nonce
advances, supply does not rise, a replay is refused with 400 and pays nothing,
and malformed hex is refused with 400. Under plain `npm test` the suite is
skipped (19 passed, 2 skipped).

**What running it against a real node found:**

1. **No gateway binary existed.** `maya-api-gateway` was a library served
   only by its own tests, so no SDK had ever reached a node through it.
   `maya2c-gateway` (`crates/api-gateway/src/main.rs`) now serves it,
   loopback by default.
2. **`send_raw_transaction` could never succeed.** The gateway deserialized
   the node's answer as a string; the node returns `{txid, accepted}`. Every
   submission through the gateway failed with 502. The recorded node in the
   gateway's tests answered a string, so its tests stayed green.
3. **A caller's mistake was reported as an outage.** A stale nonce or bad hex
   came back as 502 "upstream node error", and the SDK's `retryable` is
   `status >= 500` — so a client would retry a transaction that can never
   land. Node refusals (JSON-RPC -32602 and -32000) are now
   `GatewayError::Rejected`: 400, a fixed public message, and the node's text
   kept to the log.

Not in CI: the script builds four binaries and runs a node. It needs a
workflow job with a longer budget.
`.github/workflows/publish_sdk.yml` is dry-run until approved (Standing
Order 6) — unchanged.

## 5. Docs

`docs/site` (Starlight) regenerates its sidebar from `docs/README.md`
(`docs/site/scripts/ingest.mjs`); pages added by this work are listed there.
`cargo doc --no-deps` was not re-run for this report.
