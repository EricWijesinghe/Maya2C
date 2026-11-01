# Post-proving checklist (after `cargo xtask proving --hours 120`)

Order matters: each step runs on the **frozen sha** the proving run used, unless it says
otherwise.

**Release branch.** `release/mainnet-v1` was cut at the freeze sha `ec8d0be1` (merge of #278).
Genesis and the mainnet binaries are built from that branch, never from `master`, which moves on.
A post-freeze node fix reaches it only by cherry-pick plus a written re-proof decision:
- a small, isolated fix: attacknet on the new tip;
- anything touching consensus or the apply path: a proving restart.
Each cherry-pick and its re-proof is recorded in DECISIONS.md. A fix to node code at any step means a new freeze sha and a new proving run
(DECISIONS 2026-11-01). Calendar: `docs/mainnet-sprint.md` ("Prove", then "Launch").

| # | Step | Command / doc | Pass means | Owner |
|---|---|---|---|---|
| 0 | Proving run finished clean | `cargo xtask proving` report under `reports/` | 120 h, no halt, no fork, the gates it checks are green; paste its summary | maya2c-2b |
| 1 | Kani on the frozen sha | WSL: `cargo kani -p maya-ledger-math`, `-p maya-dex` (harnesses named in #289; `exact_out_never_undercharges` is B40), `-p maya-governance`, `-p maya-fee-market` | `VERIFICATION:- SUCCESSFUL` for each harness, each `cover!` SATISFIED | maya2c-77 |
| 2 | Attacknet on the frozen sha | `cargo xtask attacknet` (`docs/runbooks/attacknet-lab-guide.md`) | every attack's stated property holds: crash f, crash f+1 halts without fork, stolen key, wire garbage, RPC flood, long outage rejoin, restart ahead | maya2c-2b |
| 3 | Mining end to end | `cargo xtask mining-localnet`, then the Hub against that net: start mining, see shares and the on-chain "paid per hour" line move | a payout reaches the Hub's address from `MINING_RESERVE`; numbers match the chain | maya2c-77 |
| 4 | Pool at scale (non-consensus) | `pool-load` from #288 on the frozen binaries | shares/s and memory per channel within #288's report; no refused connections from the HIGH fixes | maya2c-77 |
| 5 | Restart and rejoin drills | `docs/runbooks/regenesis-checklist.md` drills; gate 8 | a validator down past the engine window rejoins with `--catch-up-from` | maya2c-2b |
| 6 | Self pre-audit round | blockchain-security-auditor + security-reviewer over the diff since the last audited sha; record in `reports/` | no CRITICAL/HIGH open; every MEDIUM fixed or written down with a reason | both |
| 7 | Release dry run | `cargo xtask release-check`; `release-binaries.yml` via `workflow_dispatch --ref release/mainnet-v1` (not master) | production build, SBOM, `SHA256SUMS`, provenance attestations verify (`gh attestation verify`) | maya2c-2b |
| 8 | Site and ledger match the build | `cargo xtask coverage --verify-targets`, `cargo xtask claims-check`, site-integrity green | no ledger entry claims more than its tests; no unbacked superlative | maya2c-77 |
| 9 | Genesis ceremony | `docs/runbooks/genesis-ceremony.md` | **needs Eric: `APPROVED: sign genesis`** | Eric |
| 10 | Tagged release + launch | tag `node-v*` → `release` environment approval (Eric) → launch | **needs Eric: `APPROVED: mainnet`** | Eric |

Rules while proving runs: no heavy local builds (the 7 validators must not starve). Node code is
verified in CI only; light `-p` checks on small crates are fine.
