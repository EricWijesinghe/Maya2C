# Launch ladder

Master Prompts 10 §7 and 20 §3. Each gate has objective exit criteria; a gate
is passed only when **every** criterion links evidence. `cargo xtask go-no-go`
reads this ladder's machine-checkable criteria and prints PASS / FAIL / NEEDS
HUMAN — never PASS without an evidence path that exists. Nothing is published,
deployed or announced without `APPROVED: <gate>` from the project owner
(Standing Order 6).

Current position (2026-09-29): **Gate 1 (localnet) passed. Gate 2's
software criteria pass: DAG-BFT finality, staking, slashing, and signing
through the remote signer (ADR-033) are in the node and tested. What
remains for Gate 2 is operational:** a persistent devnet and a 7-day soak.
A `production` build starts in `dag-bft` mode only (ADR-016, updated by
ADR-027).

## Gate 1 — Localnet

| Criterion | Evidence | State |
|---|---|---|
| Workspace builds and every test passes | `reports/raw-test-baseline-2026-09-27.log` (2,626 passed, 0 failed) | PASS |
| Validators peer, agree and survive a failure from genesis | `cargo xtask localnet`: four `maya2c-node` validators over real libp2p agree on one chain, a transfer executes on all four, three keep committing with one killed, the restarted one catches up (`reports/localnet/2026-09-28.log`, PASS in 14.7 s at `6d20648`) | PASS |
| 12-node cluster from genesis | `scripts/local_cluster.sh`; `deploy-production.sh --target local-docker` ran 12/12 RPC-healthy (not peered); `--target local-k3d` blocked in this sandbox (`reports/10-launch.md`) | partial |
| Reality ledger claims backed | `cargo xtask coverage` | PASS |

## Gate 2 — Devnet (persistent, team-operated)

| Criterion | Evidence | State |
|---|---|---|
| DAG-BFT wired into the node (ADR-015) | ADR-027; `crates/node/tests/bft_node_tests.rs` (four validators and an observer build identical chains; a double spend lands once; a validator restarted from its safety log rejoins without equivocating) | PASS |
| Staking and slashing | `crates/node/tests/bft_staking_tests.rs` (a registration joins the committee; equivocation evidence removes the validator; a stolen key is detected, slashed and replaced) | PASS |
| Fee market active (ADR-016) | `crates/node/src/state/fees.rs`: active wherever genesis configures it ("presence is activation"), checked against `maya-fee-market` limits in `genesis.rs` | built; the devnet genesis must configure it |
| Remote signer with slashing protection (Master Prompt 16) | `crates/signer`; node `--remote-signer` (ADR-033); `crates/node/tests/remote_signer_tests.rs`; `cargo xtask localnet --remote-signer` with validator 3 signing only through `maya2c-signer` (`reports/localnet/2026-09-29-remote-signer.log`) | PASS (not externally reviewed) |
| 7-day soak without an unexplained halt or fork | — | NEEDS HUMAN (a devnet must exist) |

## Gate 3 — Public testnet

| Criterion | Evidence | State |
|---|---|---|
| Spec v1 draft covers every consensus rule | `cargo xtask spec-coverage` | see `reports/15-spec.md` |
| Conformance vectors pass against node and independent verifier | `spec/tests/`, `spec/verifier/` | see `reports/15-spec.md` |
| Faucet, explorer, status page live | — | NEEDS HUMAN (`APPROVED: public-testnet`) |
| SLOs measured on staging (docs/SLO.md has no TARGET left) | — | FAIL |

## Gate 4 — Incentivized testnet + bug bounty

| Criterion | Evidence | State |
|---|---|---|
| Attacknet ran N weeks with no unresolved halt/fork | — | NEEDS HUMAN |
| Bug bounty published | `docs/security/BUG_BOUNTY.md` (draft) | NEEDS HUMAN (`APPROVED: bug-bounty`) |
| External validators ≥ target across ≥ target providers/countries, no provider above the stake-share cap | — | NEEDS HUMAN |

## Gate 5 — External audit

| Criterion | Evidence | State |
|---|---|---|
| Audit packets for scopes (a)–(d) delivered | `docs/audit/` | see `reports/20-mainnet-readiness.md` |
| Zero open critical/high findings | `docs/audit/FINDINGS.md` | NEEDS HUMAN (no audit has happened) |
| Shielded circuit externally audited | `CIRCUIT_IS_AUDITED = false` | FAIL |

## Gate 6 — Mainnet beta, Gate 7 — Mainnet

| Criterion | Evidence | State |
|---|---|---|
| Code and spec freeze; release branch; genesis parameters signed | — | NEEDS HUMAN |
| Genesis ceremony rehearsed twice on staging | `docs/GENESIS.md` | NEEDS HUMAN |
| State sync, restore and coordinated restart rehearsed in the last 30 days | `docs/runbooks/coordinated-restart.md` | NEEDS HUMAN |
| Incident response on-call rota staffed | `docs/security/INCIDENT_RESPONSE.md` | NEEDS HUMAN |
| Legal sign-off (owner confirms manually) | `docs/legal/QUESTIONS_FOR_COUNSEL.md` | NEEDS HUMAN |

## Final commands (documented, not run)

```bash
RUSTFLAGS="-C target-cpu=native -C opt-level=3" cargo build --release --bin maya2c-node --features production
maya2c-node --genesis /etc/maya2c/genesis.json --data-dir /var/lib/maya2c
```

The brief's `maya2c-node genesis launch --consensus dag-bft --pqc-level 5
--enable-pouw --gpu-miner wgpu --verify-all-proofs --exascale-mode` names
flags this binary does not have: consensus mode is a genesis parameter
(ADR-015), PoUW is deferred (ADR-016), and "exascale mode" is not a thing
this tree implements. They are not added as no-op flags.
