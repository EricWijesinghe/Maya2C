# 05 — Economic security, upgrades, operations, monitoring, incident response, formal verification, research

## 1. Economic security

Facts in force: block rewards with GPU mining as a rewards lane, no premine
(ADR-044); tail emission funded to the last height and never minted (ADR-068);
lifetime emission < 2^192; validator bond 1,000 coins; per-operator stake cap
20%; base-fee step clamp and ceiling (ADR-076); oracle optional (inv 11).

### 1.1 The 95% price-drop goal

On-chain rules never read the token price unless the optional oracle is fresh, so a
95% fall changes **no** on-chain rule. What it changes is the *cost to buy ⅓ of
stake*. The design keeps that cost above the value an attacker can extract:

| Lever | Rule (integer, price-free form) | Effect |
|---|---|---|
| Extractable value cap | Every route's `cap_epoch` in foreign units; total bridge exposure per epoch ≤ governed sum | the loot per epoch is bounded regardless of price |
| Stake-coupled cap (when oracle fresh) | `cap_epoch ≤ k · bonded_stake · price / 10_000`, `k` governed, staleness by height (inv 9); oracle stale → cap falls to a floor (fail closed) | if price falls 95%, caps fall 95% automatically |
| Unbonding brake | risk composite ≥ 6,000 doubles unbonding (03 §9) | stake cannot flee faster than an attack could be noticed |
| Slashing | equivocation slashes bond; custody misbehaviour slashes | attack costs stake even at low price |
| Committee floor | ADR-070 | liveness below a minimum committee refuses to proceed rather than shrink |

### 1.2 Death spirals, bank runs, cascading liquidations

- **No protocol-level lending or algorithmic stablecoin at L1**, so there is no
  liquidation cascade to start. The DEX is spot; a losing trade is a no-op (inv 7).
- **Progressive withdrawal throttle** (bridges and vault cells): outflow per window
  `w` limited to `max(floor, p% of the cell's assets at window start)`; `p` falls as
  the risk composite rises. Same shape as vault accounts (inv 32), applied per cell.
  Queue is FIFO and visible, so a run becomes an orderly queue.
- **Emergency reserve / insurance layer:** a governed fraction of fees (TARGET:
  10%) to an insurance reserve under `g:`; pays only via recovery transactions
  (03 §6) with governance approval; never minted. Size target: ≥ one epoch of the
  largest route's cap.
- **Dynamic security budget:** validator income = fees + rewards lane; the tail
  (ADR-068) guarantees a non-zero reward forever, so security never depends on fee
  volume alone.

## 2. Upgrade framework

ADR-082 is the framework: schedule in state (`up:`), compiled leads (inv 34),
feature gates, halt-before-fork with "upgrade required before height H",
separate rules epoch, suite sunsets with emergency flag. Additions:

1. Every new consensus feature in this TDD enters as a feature gate in a version
   entry, at `u64::MAX` until Eric writes a height (Standing Order 3).
2. Two-binary late-node rehearsal on a testnet before any real activation (the
   one check ADR-082 says is not run in one process).
3. Conformance vectors per version (`cargo xtask spec-coverage`).

## 3. Operations framework

| Area | Rule |
|---|---|
| Keys | sealed validator keys (ADR-049); ACL-locked files; never logged (Standing Order 5) |
| Releases | reproducible builds (ADR-054), provenance attestations (ADR-080), `--features production` (`release-check`) |
| Processes | kill by PID only, never by image name (memory: live seed hit 4×) |
| Changes | PR-only master, required CI, merge-when-green |
| Key person risk (RISKS R1) | runbooks for every routine task in `docs/`, a second operator trained before validators leave one machine |
| Decentralization | single-machine launch disclosed (ADR-042); operator onboarding plan after launch; 20% cap enforces it on-chain |

## 4. Monitoring framework

- **On-chain truth:** status app derives uptime and halts from block timestamps
  (features.toml note, live since 2026-10-04).
- **Node metrics:** certificate gaps, rounds/s, mempool depth, apply time, state-root
  mismatches, peer count, breaker records, halt levels, risk components.
- **Alerts:** ntfy push on halt, breaker trip, risk composite band change, root
  mismatch, validator missing > k rounds.
- **Sentinels (L0):** off-chain detectors, including ML, publish signed alerts;
  advisory only.
- **Supply chain:** `cargo deny` blocking; `cargo vet` from report-only to blocking
  for core crates (TARGET: within 3 months of mainnet), SBOM per release.

## 5. Formal verification plan

Tools in tree: `formal/kani` (10 files with proofs), `formal/lean`, `formal/z3`,
`formal/aeneas`, `fuzz/` (25 targets), `exploit_replays.rs`, proof ratchet
(`cargo xtask proof-ratchet --check`, never falls).

| Component | Spec | Invariants | Property tests | Model checking | Fuzz | Symbolic | Runtime | Safety objective | Liveness objective |
|---|---|---|---|---|---|---|---|---|---|
| ledger-math / amount | spec/03 | conservation, no overflow | proptest | Kani (exists) | yes | Kani | guard | no value created | — |
| apply / state root | spec/03 | inv 24–26 | random blocks | — | block fuzz | — | guard, root check | determinism | progress per block |
| DAG-BFT | spec/05 | no two conflicting commits | simulation | **TLA+ or Lean model of commit rule (to write)** | message fuzz | — | equivocation detector | agreement | commit after GST |
| Halt levels | 03 | P-2, P-3, P-5 | all-level matrix | Kani on `tx_allowed` | action fuzz | Kani | level records | S1–S3 | S4 (expiry) |
| Risk engine | 03 §9 | P-7 | two-DB determinism | Kani on integer formulas | state fuzz | — | — | no panic, no overflow | recompute each epoch |
| Custody (FROST/ROAST) | 04 §2 | inv 18, 19 | share tests | — | share fuzz | — | reconciler | unforgeability (reduction cited) | ROAST bound |
| Bridge books | 02 §6 | P-6 | forged proofs | Kani on cap logic | proof fuzz | — | guard books | minted ≤ locked | refunds always payable |
| Upgrade schedule | ADR-082 | inv 34 | schedule fuzz | — | yes | — | halt check | no early activation | halt at H |

## 6. Incident response and recovery frameworks

| Phase | Action | Owner |
|---|---|---|
| Detect | alert (L0) or rule (L1–L3) | sentinels / guard |
| Contain | raise the lowest sufficient level on the smallest cell | rule, or council + stake |
| Communicate | status page + signed notice; no blame, no speculation | operator |
| Eradicate | fix as PR; replay test first | developers |
| Recover | forward-only recovery tx (03 §6), controlled restart via ADR-082 | governance |
| Learn | `reports/incidents/<date>.md`; exploit becomes a test; level lowered only after | operator |

Runbooks: one per level, each with the exact command and the check that proves it
worked. Drill each level on testnet before its activation height is set.

## 7. Remaining research problems

1. Standardized threshold PQ signatures (our own keys, problem A).
2. STARK aggregation of hybrid tx signatures per block at acceptable prover cost.
3. Lazy certificates: liveness argument under adversarial vote withholding.
4. State-growth bound (rent vs deposit) without breaking small-account UX.
5. Data-availability sampling with PQ commitments (lattice or hash based).
6. Light-client proofs for foreign chains that are themselves not PQ.
7. Checkpointing to Bitcoin (Babylon-style) under a PQ threat model.
8. Sharded execution (blockgraph) with cross-shard atomicity under inv 24–26.
9. ML sentinels with a measured false-positive rate low enough to be useful as L0.
10. A competitor matrix with dated sources (`docs/prior-art/`) so the "beyond" claim
    can be made publicly, row by row.
