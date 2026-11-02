# Mainnet readiness assessment, 2026-11-02

Assessed against `origin/master` at `7bbf45f6`, with the ten open PRs (#308, #313,
#315–#322). Sources: `features.toml` (164 features, 156 subsystems),
`docs/site/src/data/mainnet-gates.ts`, `.claude/plans/master-ledger-2026-11-01.md`,
`reports/11-gap-register.md`, `docs/launch-checklist.md`, `docs/mainnet-sprint.md`,
`docs/runbooks/`. Nothing was built or run for this report. Every status below names a
file. "NOT VERIFIED" means nobody has checked it, not that it is broken.

No new features are proposed here. Anything that would be new is listed under
Stage 7 with the decision that deferred it.

## 0. Where the brief and the project disagree (Standing Order 9)

| Brief asks for | What is already decided | Effect on this plan |
|---|---|---|
| Alpha, beta, public and incentivized testnets | Eric 2026-10-28 and 2026-11-02: no second long soak. The 7-day soak passed (gate 3, 7 d 15 h, 0 halts). Proving runs and temporary nets last 24 h at most (PR #322). The next public network is mainnet. | Stages 1–3 are **already passed**. They are not run again |
| Third-party security audit | Eric 2026-10-28: no third parties until mainnet. ADR-042: self pre-audit, labelled as such | Stage 4 is a self-audit. Never call it independent |
| "Zero unknowns, zero placeholders" | Some unknowns only end with time or money: uptime history, an external audit, Authenticode | They are listed as known, owned and dated. They are not hidden |
| Bridges, identity, mobile wallet, browser wallet at launch | ADR-016 launch scope; bridges ADR-065 Proposed; identity RESEARCH; no mobile or browser wallet app exists in `apps/` | Not launch blockers. Stage 7 |
| Treasury and foundation budget | ADR-044: no premine, rewards only. The treasury mechanism exists (`MAX_TREASURY_SHARE_BPS` = 50%), but is not used at genesis | Genesis treasury share = 0. Funding: `funding-free-path` |

## 1. Protocol components (Phase 1)

Status: **C** complete (REAL + verified, test named in `features.toml`), **P** partial,
**M** missing, **R** needs redesign.

| Component | Status | Evidence | What is left before mainnet |
|---|---|---|---|
| Consensus (DAG-BFT, Narwhal + Bullshark) | P | `features.toml` verified; gates 8 and 10 met (attacknet) | #317 re-freeze: a ChainTag in `channel_state_signing_bytes`, the CON-13 payload share, nonce before verify, the builder Err. Long-range rejoin B49 (P1 before any outside validator). Proving run of 24 h on the frozen sha |
| Mining (DAG-PoW, GPU) | P | Rewards lane REAL **working**, not verified; CUDA/wgpu verified | Gate 6 "running": the forever tail and fee burn shipped? → check #221 on master. WorkClaim refused from genesis (#317, B50). Hub end-to-end mining on the frozen build |
| Networking (libp2p, PQ Noise) | P | libp2p verified; ML-KEM-1024 transport **working**; dual-KEM `off` | #298 non-routable Kademlia needs an attacknet re-proof. #316 RPC client IP. The mainnet bootnode peer id in every join path (B27) |
| Wallets | P | CLI, wallet core and Tauri GUI verified; Ledger **working** (Speculos suite never run) | #318 sign hardening. #135 send-screen check by Eric. Default a real `min_out` (B38). Ledger stays labelled a spike |
| Governance | C (scope cut) | lifecycle verified; stake-only at launch (#317) | ADR-050 council: maximum pause and the offline key location (Eric, B30) |
| Security | P | gap register 0 P0 / 0 P1 (#312); self pre-audit found 2 HIGH + 4 MEDIUM | #315, #316, #318 merged; B1/B2 Hub TOCTOU + Windows DACL; B16 fuzz (#313) |
| Recovery | P | runbooks for halt, state-root mismatch, deep reorg, key compromise; gate 10 met | SRN (ADR-077) is post-mainnet by decision. Run a restore-from-snapshot drill on the frozen build |
| Storage / pruning | C | pruning + archive bootstrap verified | B17 framing caps. Arweave left out on purpose (it costs AR) |
| Execution engine | P | WASM Cranelift verified; Pulley tiers + parallel exec RESEARCH | Launch uses the verified path only. Confirm the RESEARCH paths are behind `u64::MAX` heights in the genesis config |
| Virtual machine | C | as above | — |
| Identity | M (out of scope) | RESEARCH | Stage 7 |
| Bridges | M (out of scope) | HTLC swaps verified; bridge routes ADR-065 Proposed | HTLC only at launch; `htlc-watcher` per-connection genesis cache (B12) |

Nothing is marked **R**. Rewriting a working subsystem is not allowed without Eric.

## 2. Codebase (Phase 2)

The structure exists: 87 workspace members and 11 outside crates
(`docs/workspace-map.md`). Services needed at launch, all in the tree:

| Service | Crate | State |
|---|---|---|
| Validator / full node | `bins/maya2c-node` | frozen build pending #317 |
| Developer and wallet CLI | `bins/maya2c-cli`, `l1-wallet` | shipped in the release (#209) |
| Pool | `bins/pool-service` | B35–B37, B39, B44 follow-ups |
| Explorer + indexer | `apps/explorer` | new pages written but not deployed. No kind column (STATE 10-27) |
| Status / uptime | `apps/status` | working, not verified |
| Hub (mining GUI) | `apps/hub` | B1, B2, B41 (signed updates) |
| Desktop wallet | `apps/wallet-gui` | #318, #311 CSP |
| Updater | `apps/app-update` | the node has no notify-only channel yet (B4) |
| Faucet | `apps/faucet` | **must not run against mainnet** (it refuses one) |
| Monitoring | Prometheus rules in `infra/k8s/base` | alert channel unset (B29) |

Missing code that blocks launch: **none of it is new**. Every item is a finish or a fix
on the B list.

## 3. Cryptography (Phase 3)

| Item | Status | Left |
|---|---|---|
| Signatures: hybrid ML-DSA-65 + SLH-DSA-SHA2-128s; ML-DSA-87 validator keys (#222) | C | C15 doc drift: `consensus.mdx` says ML-DSA-65 for votes |
| Hashing: BLAKE3, SHA3 | C | — |
| Key management: sealed validator keys (ADR-049) | P | The live keys were plaintext; mainnet keys are sealed when generated. BitLocker off (B28). Ceremony `*.secret` custody (launch-checklist item 4) |
| Wallet recovery: mnemonic + derivation (`wallet-gui/core`) | C | A restore test from mnemonic alone on each OS: NOT VERIFIED |
| Randomness: ML-DSA seed-only RNG (the `unimplemented!` in keys.rs is a reviewed guard) | C | — |
| Encryption: ML-KEM-1024 transport, threshold mempool | P | dual-KEM stays `off` until `docs/pq-transport.md` holds |
| Formal proofs | P | B10 u256 Kani in WSL; B33 harness 2 (14 h run); B40 nightly Kani stall |

Specs that are missing: none known. `cargo xtask spec-coverage` on the frozen sha is
what confirms that. NOT VERIFIED today.

## 4. Node roles (Phase 4)

| Role | Exists | Requirements and protocol | Left |
|---|---|---|---|
| Bootstrap / seed | yes (`infra-live` routes, bootnode multiaddr) | libp2p over a tunnel, WebSocket P2P | A fresh mainnet peer id, published everywhere (B27) |
| Validator | yes, foundation set (ADR-047) | sealed ML-DSA-87 key, BFT | B7: 3 hosts + a 4th (Eric) |
| Archive | yes (pruning off = archive) | full block store | Name one host as the archive |
| Light | SPV light client verified | headers + proofs | — |
| Mining | Hub + pool + stratum v2 | stratum v2 to the pool, `submit_block` to the node | Hub end to end on the frozen build |

Hardware requirements for operators: `docs/runbooks/foundation-validators.md`. Re-check it
against the u256 build sizes.

## 5. Mining (Phase 5)

Mining software ✓ (Hub, CUDA, wgpu). Pool protocol ✓ (stratum v2). Difficulty
adjustment ✓ (consensus). Reward distribution: gate 6 running. Before the first miner arrives:

1. Gate 6 closed: the forever tail (#221) and the burn are on master. Left: B43 (21M at
   10^8 vs a 24-decimal coin, in the ADR's wording) and the fee-treasury share (§13).
2. A5: Eric's wording on the ADR-044 reserve.
3. Pool load fixes B37 (a third of 10k unpaced connects refused) and B39/B44 follow-ups.
4. Anti-centralization: a 20% cap (mainnet plan 10-25); per-/64 rate buckets (#309).
5. Hub 2-click mining, signed (B41). Authenticode is money (B14, Eric). Until then,
   follow `code-signing-free.md`.
6. Mining SDK: the pool protocol is the SDK. The CUDA telemetry gap (launch-checklist 8)
   is open. Check whether it is stale.

## 6. Validators (Phase 6)

Requirements, lifecycle and penalties are in consensus (bond, unbonding window, slashing
via bonds). Recovery: the `fell-behind`, `coordinated-restart` and `stuck-upgrade`
runbooks. Upgrade process: header version byte (ADR-063) plus coordinated restart. Left:
B49 long-range rejoin, B4 update channel, ADR-050 council values (Eric), gate 4
(publicly stated single-operator launch, ADR-047).

## 7. Wallets (Phase 7)

| Wallet | State |
|---|---|
| Desktop (Tauri) | verified; #318, #311, #135 open |
| CLI | verified |
| Hardware (Ledger) | spike; Speculos suite never run; say so in copy |
| Mobile, browser | not in the tree. Post-mainnet (platform targets: Android later) |
| Address standard | `blake3(v3 ‖ ml_dsa_pk ‖ slh_dsa_pk)`, `normalize_address` / `decode_address` |
| QR | `PaymentRequest` + QR SVG test (#311) |
| Backup / recovery | mnemonic; a cross-OS restore drill is NOT VERIFIED |

## 8. Explorers (Phase 8)

Block, transaction and address views exist (`apps/explorer`). Validator view: `apps/status`
BFT view. Left: deploy the redesign (#137), add the kind column via `core::describe`,
screenshot diffs in CI (C10). Governance, treasury and security dashboards: not built, and
not needed at launch (governance is stake-only, and there is no treasury). Stage 7.

## 9. Developers (Phase 9)

SDKs verified (multi-language). Kotlin and Swift are generated and never compiled
(launch-checklist 5). Either compile them or mark them unsupported. Rosetta/Mesh API is
working. RPC docs: run `cargo xtask spec-coverage` and the doc-links check. The
`dev-hub` and `portal` apps exist. Left: SDK docs pass (C16-10), `docs/reference.tex`
compile or delete the claim (launch-checklist 2).

## 10. Test networks (Phase 10)

Already done: the public testnet (7 d soak, gate 3), the attacknet (all 8 attacks, gates 8
and 10), 7-validator stagenet with WAN netem, adversarial BFT schedules (10-28).

Left, each 24 h or less on the **frozen** sha:

- [ ] proving run of 24 h (`xtask proving --hours 24`, #322)
- [ ] attacknet re-proof (#298, #316 changed node code)
- [ ] Hub mining end to end
- [ ] restart + rejoin drill, snapshot restore drill
- [ ] release dry run from a tag

## 11. Audit plan (Phase 11, self pre-audit per ADR-042)

| Plan | Scope | State |
|---|---|---|
| Code | the gap register, clippy ratchet, `cargo deny`, fuzz (#313) | 0 P0/P1; fuzz first CI run pending |
| Protocol / consensus | #317 findings; Kani; conformance vectors | HIGH being fixed |
| Economic | DEX H-1/H-2, B33, B38, ADR-071 fee ceiling | B33 harness 2 running |
| Governance | stake-only launch, ADR-050 council | Eric values |
| Recovery / DR | gate 10, DR drill (10-19 queue) | re-run on the frozen build |
| Supply chain | `cargo vet` report-only (1,068 exempt), SBOM + provenance (#130) | stays report-only. Disclose it |

External audit: post-mainnet, free routes first (C16-5). Never claimed before it happens.

## 12. Attack simulation (Phase 12)

| Attack | Covered by | Left |
|---|---|---|
| 51% / hashpower | DAG-PoW + BFT finality; attacknet | — |
| Validator collusion (f+1) | attacknet crash f+1, stake-weighted committees | B49 |
| Bridge hack | no bridge at launch | — |
| Exchange breach | out of protocol; replay protection gate 1 | listing packs (C14) |
| Contract exploit | WASM gas determinism, DEX Kani | B33, B38 |
| Network partition / routing | netem stagenet, eclipse runbook | #298 re-proof |
| State corruption | invariant 24, `state-root-mismatch` and `db-corruption` runbooks | snapshot restore drill |
| Recovery failure | gate 10 | re-run frozen |
| Governance attack | stake-only, bounded parameters | ADR-050 |

## 13. Tokenomics (Phase 13)

Genesis allocation: **no premine** (ADR-044). Treasury and foundation budget: 0 at genesis.
Mining rewards: block rewards from block 1 and a tail that never ends. Fees: 0.1 MAYA
ceiling (ADR-071 B, A4 needs Eric's sign-off), part of every fee burned. Validator
incentive: fees-only (mainnet decisions 10-04). Supply: 21M (DECISIONS 10-28), money is
u256. **Open:** B43, the 21M at 10^8 vs 24 decimals wording. **Open:**
`crates/fee-market/src/config.rs:71` defaults to an 80/20 burn/treasury split of the
base fee, while genesis has no treasury. Either the mainnet config sets the treasury
share to 0 or it names who holds that 20%. That is Eric's call (economics). Sustainability is an
economics decision. It belongs to Eric and is not changed here.

## 14. Operations (Phase 14)

Monitoring: Prometheus + status page. Alerting: **missing channel** (B29). Incident
response: 30 runbooks in `docs/runbooks/`. Emergency team: Eric + Claude (no third party
by decision), council keys (B30). Upgrade / fork: ADR-063 version byte,
`coordinated-restart.md`, `stuck-upgrade.md`. Support: site FAQ plus a GitHub issues
template. The template is NOT VERIFIED.

## 15. Mainnet checklist (Phase 15)

Effort is a size, not a measurement: S ≤ half a day, M 1–2 days, L more than that.
Owner: **E** Eric, **77** this lane, **2b** the peer session, **CI**.

| # | Task | Pri | Diff | Depends on | Owner | Effort |
|---|---|---|---|---|---|---|
| 1 | Merge #317 consensus pre-audit; re-freeze; move `release/mainnet-v1` | P0 | M | reviews | 2b | M |
| 2 | Fold #298 and #316 into the freeze | P0 | S | 1 | 2b | S |
| 3 | Ruleset for `release/*` (B45) | P0 | S | — | E | S |
| 4 | Full nextest + `coverage --verify-targets` + spec-coverage on the frozen sha (B11) | P0 | S | 1 | CI | S |
| 5 | Proving run of 24 h on the frozen sha | P0 | S | 1 | 2b | 24 h |
| 6 | Attacknet re-proof on the frozen sha | P0 | M | 1 | 2b | S |
| 7 | B33 Kani harness 2 verdict (a FAIL means a re-freeze) | P0 | S | — | 77 | running |
| 8 | Gate 6: forever tail (#221) and burn are on master. Left: B43 wording + the 20% fee-treasury share with no treasury | P0 | S | — | E | S |
| 9 | A4 fee ceiling, A5 reserve wording | P0 | S | — | E | S |
| 10 | Merge #318 wallet hardening, #315 updater key, #311 CSP | P1 | S | CI | 2b | S |
| 11 | B49 long-range rejoin (before any outside validator) | P1 | M | — | 77 | M |
| 12 | B1/B2 Hub TOCTOU + Windows DACL | P1 | M | — | 77 | M |
| 13 | #313 fuzz targets, first CI run | P1 | S | CI | 77 | S |
| 14 | B12, B17, B18 node/watcher fixes | P1 | S | — | 77 | M |
| 15 | Pool: B37 cause, B39/B44 follow-ups | P1 | M | — | 77 | L |
| 16 | B27 mainnet bootnode id everywhere; regenerate the validators guide | P1 | S | 21 | 77 | S |
| 17 | B4 node notify-only update channel | P1 | M | — | 77 | M |
| 18 | B41 Hub signed-update verify by workflow_dispatch | P1 | S | 1 | 77 | S |
| 19 | Release dry run from a tag, reproducible build check | P1 | M | 1, 3 | CI | S |
| 20 | B7 hosts (3 + 4th); B28 BitLocker/page file; B29 alert channel; B30 council | P1 | S | — | E | S |
| 21 | Genesis ceremony: generate sealed ML-DSA-87 keys, reproduce byte-identical, move `*.secret` to custody | P0 | M | 1–9, 20 | 77 + E | S |
| 22 | `APPROVED: sign genesis` | P0 | — | 21 | E | — |
| 23 | Deploy explorer + status redesign; kind column | P2 | M | 21 | 77 | M |
| 24 | C15 doc drift; Kotlin/Swift compile or mark unsupported; `reference.tex` | P2 | S | — | 77 | M |
| 25 | Re-check `docs/launch-checklist.md` "Outstanding" items 1–12 for staleness | P2 | S | — | 77 | S |
| 26 | Cross-OS mnemonic restore drill; snapshot restore drill | P2 | S | 1 | 77 | S |
| 27 | B15 bench-tps on the frozen build (commit, hardware, command) | P2 | M | 1 | 77 | S |
| 28 | B5 Cloudflare settings, B31 incident notice | P2 | S | — | E | S |
| 29 | `APPROVED: mainnet` | P0 | — | 22, 5, 6 | E | — |
| 30 | B14 Authenticode / AV submissions (costs money) | P3 | S | — | E | S |

## 16. Roadmap

**Stage 1, before alpha: PASSED.** Devnets, conformance vectors, gap register at 0 P0.
**Stage 2, before beta: PASSED.** Attacknet gates 8 and 10, stagenet, adversarial schedules.
**Stage 3, before testnet: PASSED.** 7-day public soak, gate 3. No second soak (Eric).

**Stage 4, before the self pre-audit closes:** items 1, 2, 7, 10, 11, 12, 13, 14, 15.
**Stage 5, before genesis:** items 3, 4, 5, 6, 8, 9, 16–21, 25, 26, then 22.
**Stage 6, before mainnet:** items 23, 24, 27, 28, then 29. Gates 4–7 show "met" with
evidence on the site.

**Stage 7, after mainnet (decided, not new):** SRN R1–R7 (ADR-077); Omega moats (ADR-078);
external audit, free routes first; bridges (ADR-065); identity; mobile and browser
wallets; governance, treasury and security dashboards; shielded pool after an audit
(ADR-037); ADR-062a/b; dual-KEM `on`; `cargo vet` blocking; uptime history from block 1;
Authenticode if not done; macOS/iOS; ledger D1–D14.

## What this report did not check

- No command was run. Every "C" relies on `features.toml`, and `cargo xtask coverage`
  is what enforces it. The frozen-sha run is item 4.
- Item 25 (launch-checklist staleness) was found while writing this report, not
  checked. Item 8 was checked: #221 merged 2026-10-28, and the burn is in
  `fee-market/src/execute.rs`.
