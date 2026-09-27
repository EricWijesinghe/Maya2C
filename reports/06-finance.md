# 06 — DeFi, finance, identity and the physical world

**Machine:** cloud VM, 4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4,
`nightly-2026-07-15`. Test lines are from the 2026-09-27 full workspace run
(`--profile ci`) unless marked as run for this report.

## Verdict

DONE WHEN: *every module has tests, a `features.toml` entry with its class,
and registered invariant hooks; this report is complete.*

| Module (brief §) | Tests | Ledger entry | Invariant-guard hook | State |
|---|---|---|---|---|
| DEX / AMM + batch auctions (§1) | `dex_tests.rs` 27 ✓, `crates/dex/tests/*` | yes | yes (AMM reserves, invariants 6–7) | met |
| Payment channels "Maya Flash" (§2) | `channel_tests.rs` 26 ✓, `scale_tests.rs` 4 ✓ (1 ignored: 20k signatures, ~1 h) | yes | yes (channel layer under the state root) | met; machine barter / 10k IoT devices not built |
| Cross-chain (§3) | **new** `crates/btc-spv` 9 ✓; `interop::beacon` (Ethereum sync-committee light client, verified on a real mainnet fixture — `reports/25-interop.md`) | yes | n/a (not on chain) | partial: Bitcoin SPV with 6-block reorg test and an Ethereum light client; lock/mint not built |
| ISO 20022 (§4) | `iso20022_tests.rs` 20 ✓ | yes | n/a (bridge) | met for parsing/generation; types hand-written, not generated from XSDs |
| RWA (§4) | `rwa_tests.rs` 11 ✓ | yes | yes | `ten_thousand_dividends_settle_in_one_block` (the brief's number) and DvP present |
| CBDC / dark pool (§4) | **new** `crates/permissioned-finance`: `vault_tests.rs` 3 ✓, `compliance_tests.rs` 4 ✓ | yes (new, RESEARCH) | n/a (not on chain) | built — §4 below; the MPC form of the dark pool is not |
| Compliance + tax (§5) | sanctions and credential STARK statements in `crates/zk-stark`; **new** `permissioned_finance::tax` | yes (new) | — | US/UK/DE calculators and `compliance_tests.rs` (500 transactions) built; `docs/LEGAL_NOTICE.md` applies |
| Macro-economic engines (§6) | **new** `econ/` 6 ✓ | yes (new, SIM) | n/a (simulation) | built as an agent-based simulator first, as the brief requires |
| Identity (§7) | `identity_tests.rs` 15 ✓ | yes | yes | DID + attestations + selective disclosure; EEG/BCI SIM and PoP biometrics not built |
| DePIN / IoT (§8) | `iot_anchor_tests.rs` 7 ✓ | yes | yes | enrollment → telemetry → tamper; energy protocol parsers, satellite NDVI oracle, swarms not built |
| Oracle (§8) | `oracle_tests.rs` 30 ✓ | yes | yes (invariant 9) | quorum median (one liar cannot move it) and stale-feed refusal present; a dispute window is not |
| Settlement | `settlement_tests.rs` 20 ✓ | yes | yes | — |

## 1. Bitcoin SPV (new)

`cargo test -p maya-btc-spv` — 4 unit + 5 integration tests pass. Highlights:

- **Real mainnet headers 0, 1, 2** hash to the published block hashes, meet
  their targets, and chain (`real_mainnet_headers_hash_meet_their_targets_and_chain`).
- Compact-target encoding matches Bitcoin Core's `arith_uint256` vectors,
  including the negative and overflow encodings Core rejects.
- Retarget: on-schedule keeps the target, the 4× clamp holds both ways, the
  pow limit caps it.
- **6-block reorg:** a deposit 6 deep on the honest chain disappears when an
  attacker publishes 7 blocks from height 4, and returns when the honest chain
  regains the lead. Confirmation depth is a policy the bridge states, not a
  proof.
- Merkle proofs hold at every position for 1–19 transactions and refuse a
  64-byte "transaction" and the duplicate-node shape (CVE-2012-2459).

A real-chain retarget vector was deliberately not included: it would need the
timestamps of blocks 30,240 and 32,255 taken on trust.

## 2. Macro-economic engine (new, SIM)

`econ/` — see `reports/18-economics.md` for the full run. The brief's three
shocks, from a funded starting point (1.5× the break-even price for 100
validators; the draft's own 1.0 input funds only 4):

| Shock | min staking | validators (min) | supply change | stable |
|---|---|---|---|---|
| 30% currency devaluation (costs × 1.43) | 50.3% | 100 (100) | +1.35% | yes |
| 50% market crash | 50.3% | 74 (74) | +1.32% | yes |
| liquidity shock (demand −90% 60 days, 20% unstake) | 50.3% | 102 (100) | +2.11% | yes |

"Stable" = staking ratio ≥ 1/3 and ≥ 2/3 of the initial set solvent on every
day, supply ≤ `MAX_SUPPLY`. **No module guarantees price stability**; prices
are scenario inputs. Real-time ZK proof-of-reserves is not built.

## 3. Not built

Dark-pool matching under MPC (orders here are hidden until the batch
closes, then revealed — §4); generic L1/L2 light-client framework; energy protocol parsers (Modbus/TCP, IEC 61850, IEEE
1547) and grid SIM; satellite NDVI oracle; robot swarm auctions; EEG/BCI SIM;
biometric proof-of-personhood; machine-to-machine barter with 10,000 devices.

## 4. Permissioned finance (new, RESEARCH) — run for this report

Run on the Windows workstation, 2026-09-27, `nightly-2026-07-15`.

`cargo test -p maya-permissioned-finance -- --nocapture`: 7 passed
(`compliance_tests` 4, `vault_tests` 3), 0 failed.

- **CBDC vault** (`cbdc.rs`). Admission verifies a STARK proof
  (`zk-stark::credential`) that some unrevoked credential in the KYC issuer's
  tree carries a tier ≥ the one requested. Tested: a tier-1 holder cannot
  produce a tier-2 proof, a revoked credential cannot be proved, a tier-2 proof
  does not admit at tier 3, and a proof against another issuer's roots is
  refused. Then `fifty_thousand_compliant_settlements_conserve_supply`:
  `50000 settlements in 12.1 ms (4121774/s)` in a debug build, supply equal to
  the sum of balances afterwards. Per-tier limits, freezes and issuer-only
  minting each have a refusal test.
- **Dark pool** (`darkpool.rs`). Orders enter as `BLAKE3(order ‖ salt)`
  commitments; the book is commitments only until the batch closes. Reveals
  must open their commitment; unrevealed orders do not trade. Clearing is at the
  one price that maximises matched volume (lowest on a tie). The 500-order test
  checks the price against a brute-force auction, bought = sold, and that no
  unrevealed trader was filled. `cargo bench --bench darkpool_bench` (release):
  `darkpool 1000 orders (best of 20): commit 0.323 ms, reveal 0.376 ms, clear
  0.129 ms, total 0.828 ms; price Some(100)`.
- **Tax** (`tax.rs`). US (FIFO, short/long split at 365 days), UK (single
  average-cost pool), DE (FIFO, lots held > 365 days exempt), integer cents.
  500 generated trades: US and DE gains sum exactly to net cash; the UK pool is
  within one cent per disposal of it (allowable cost rounds down). A
  hand-worked case pins each jurisdiction. Not tax advice.

**Limits, stated:** the dark pool is commit-reveal, not MPC — prices and sizes
are public after the batch closes. The KYC proof is not bound to the account
being admitted: the credential circuit keeps the subject private and exposes
no public input to bind it to, so a proof could be replayed by another account.
Closing that needs a subject-commitment public input in
`zk-stark::credential`, which changes a circuit other statements use; it is
listed here rather than patched around.

