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
| Cross-chain (§3) | **new** `crates/btc-spv` 9 ✓ | yes (new) | n/a (not on chain) | partial: Bitcoin SPV with 6-block reorg test; Ethereum light client and lock/mint not built |
| ISO 20022 (§4) | `iso20022_tests.rs` 20 ✓ | yes | n/a (bridge) | met for parsing/generation; types hand-written, not generated from XSDs |
| RWA (§4) | `rwa_tests.rs` 11 ✓ | yes | yes | `ten_thousand_dividends_settle_in_one_block` (the brief's number) and DvP present |
| CBDC / dark pool (§4) | — | — | — | **not built** |
| Compliance + tax (§5) | sanctions and credential STARK statements in `crates/zk-stark` (`sanctions.rs`, `statement_tests.rs`) | partial | — | tax calculators, `compliance_tests.rs` (500 tx) **not built**; `docs/LEGAL_NOTICE.md` **added** |
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

CBDC/permissioned vaults with ZK-KYC; dark pool with MPC sealed bids and
`benches/darkpool_bench.rs`; per-jurisdiction tax calculators and
`compliance_tests.rs`; Ethereum sync-committee light client; generic L1/L2
light-client framework; energy protocol parsers (Modbus/TCP, IEC 61850, IEEE
1547) and grid SIM; satellite NDVI oracle; robot swarm auctions; EEG/BCI SIM;
biometric proof-of-personhood; machine-to-machine barter with 10,000 devices.
