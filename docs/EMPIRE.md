# The milestone ladder

Every Master Prompt (MP) 1–30 belongs to exactly one milestone. A milestone
is met only when **every** exit criterion below it has evidence on disk,
and nothing advances past a milestone whose criteria are not met and
recorded in [STATE.md](../STATE.md). Status words: **Met** (evidence linked),
**Open** (work remains), **NOT VERIFIED** (the claim may be true but no
evidence was checked), **Owner** (needs Eric: money, people, hardware or a
decision).

MP status is taken from [master-prompts/README.md](master-prompts/README.md)
and the numbered reports, as of 2026-09-28. It is re-derived at each
verification sweep; a downgrade is recorded, not hidden.

## Master Prompt → milestone

| MP | Title | Milestone | MP status (README) |
|---|---|---|---|
| 01 | Foundation | M1 | Met |
| 02 | Cryptography, ZK, custody | M1 | Met, with findings |
| 03 | State, storage, economics | M1 | Met |
| 04 | Consensus, mining, scaling | M1 | Met |
| 05 | VM, contracts, on-chain AI | M1 | Met |
| 06 | DeFi, finance, identity, physical | M1 (core parts only — see question Q1) | Partial |
| 07 | Networking, transports, time | M1 | Met (eBPF load in its own CI job) |
| 08 | Security, formal verification | M1 | Met; line coverage not measured |
| 09 | Governance, wallets, explorer, SDKs | M3 | Partial |
| 10 | Infrastructure, release, genesis | M5 (genesis signing: M8) | Met |
| 11 | Production baseline, scope freeze | M0 | Met |
| 12 | Execution performance | M1 (baseline) and M2 (the rest) | Partial |
| 13 | PQ weight | M2 | Met |
| 14 | State sync, light clients, RPC | M2 | Met |
| 15 | Protocol spec, conformance, upgrades | M5 | Met |
| 16 | Validator and key security | M5 | Met |
| 17 | Integration layer | M4 | Partial |
| 18 | Economic security, launch economics | M5 | Met |
| 19 | Operations, public testnet | M5 (operations) and M6 (testnet) | Met (engineering) |
| 20 | Mainnet readiness | M7 (audits) and M8 (go/no-go) | Met (verdict NO-GO) |
| 21 | Weakness map, beat bars | M6 | Met |
| 22 | Accounts without pain | M3 | Partial |
| 23 | Safe-by-default contracts | M3 | Met (VM, opt-in) |
| 24 | Developer platform | M3 | Partial |
| 25 | Interop without trusted bridges | M4 | Partial |
| 26 | Scale without fragmentation | M2 | Met |
| 27 | Privacy primitive, compliance | M4 (differentiator; activation after M7) | Partial |
| 28 | Quantum-safe harbor | M4 (differentiator) | Partial |
| 29 | Wallet, explorer, portal UI | M3 | Partial |
| 30 | Adoption engine, public proof | M6 | Partial |

MP 9, 10, 14, 18, 27 and 28 are not named in the operating prompt's
ladder; their placement above is a proposal (DECISIONS.md, 2026-09-28).

## Exit criteria

### M0 TRUTH — **Met** (re-confirmed by each sweep)
- [x] MP11 DONE WHEN met — `reports/11-reality-audit.md`, 2,626 passed / 0 failed
- [x] Gap register generated — `reports/11-gap-register.md`. Its 3 P0 rows were
      reviewed 2026-09-28: all three are deliberate guards (a `const fn` panic
      evaluated at compile time; two `RngCore` methods `fips204` never calls),
      not missing work.
- [x] Reality ledger gated — `cargo xtask coverage --verify-targets` in CI
- [x] Scope frozen — [ADR-016](adr/ADR-016-launch-scope.md)

### M1 CORE SOLID — **Open** (current milestone)
1. Clean `cargo build --workspace --all-targets` — sweep
2. `cargo nextest run --workspace`: 0 failed — sweep
3. fmt, clippy gate, unsafe accounting, lint ratchet, doc coverage — sweep
4. CI green on `master` — observed on GitHub
5. Property tests for state, fees, consensus, VM — state: `crates/node/tests/supply_property_tests.rs` (seeded); fees, consensus, VM: **NOT VERIFIED** (no property-test file found by search 2026-09-28)
6. Fuzzing of every consensus decoder — 16 targets in `fuzz/fuzz_targets/`, CI fuzz jobs green on PR #1
7. Formal: Kani proofs run in CI — green on PR #1; invariants proven or listed open — `docs/invariants.md`, `reports/08-security.md`
8. Line coverage of core crates measured — **Open**: never measured (`cargo-llvm-cov` 0.8.7 is installed, not yet run)
9. MP12 baseline recorded — `reports/12-baseline.md`
10. MP06's mainnet-core parts done — **Owner** (Q1)

### M2 FAST AND PROVEN — Open
- MP12 regression gate runs on a dedicated runner — **Owner** (`BENCH_RUNNER`)
- MP13, MP14, MP26 reports carry commit, hardware, OS and command for every number — `reports/13-*`, `14-*`, `26-*` (to re-check at the next sweep)
- A `cargo xtask bench` exists — **Open** (CLAUDE.md, Production Standing Orders)

### M3 USABLE — Open
- MP22 and MP29 usability studies with outside participants — **Owner**
- MP24 developer study — **Owner**
- MP09 wallet, explorer, SDKs end to end — SDK e2e and wallet suite pass (`reports/09-product.md`)
- MP23 safe-contract rules on the consensus import surface — **Open** (opt-in only today)

### M4 CONNECTED — Open
- MP25 proof-verified interop (not hash-locked only) — **Open**; ZK light client is research
- MP17 Mesh `check:construction` — blocked by Mesh having no PQ signature type
- MP27, MP28: built parts reviewed; activation waits for M7

### M5 OPERABLE — engineering Met, not re-verified
- MP15 conformance: 71 vectors, 0 gaps — CI job observed green on PR #1
- MP16 remote signer, slashing protection — `reports/16-validator-security.md`
- MP10/MP19 operator runbooks rehearsed — `reports/19-operations.md`
- MP18 economics — `reports/18-economics.md`

### M6 PUBLIC TESTNET — Owner
Public network, outside validators, attack round, beat bars measured
against a live network. Needs `APPROVED:` for spend and deploy.

### M7 AUDITED — Owner
External audits of consensus, crypto-pq, VM host functions, custody,
privacy circuits; no open critical or high finding.

### M8 MAINNET — Owner
`cargo xtask go-no-go` all PASS (today: 6 PASS, 6 FAIL, 4 NEEDS HUMAN);
signed genesis; 90-day plan.

### M9 ECOSYSTEM — planned in [ECOSYSTEM.md](ECOSYSTEM.md)
Chat is deliberately allowed to ship before M6 because it does not need
the chain (ECOSYSTEM.md §2). That is the one exception to the ladder's
ordering, and it is Eric's decision.

## Open questions for Eric

- **Q1.** MP06 is mostly physical-world and frontier work that ADR-016
  defers. Does M1 require all of MP06, or only the parts inside the ADR-016
  mainnet core (CBDC vault, tax, compliance)? Recommended: only the core
  parts; the rest stays P6.
