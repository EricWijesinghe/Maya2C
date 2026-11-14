# ADR-079: Verifiable compute: useful work for AI, science and multi-GPU hardware

**Status:** Proposed, 2026-11-03. Build after mainnet (ADR-016, ADR-069). Nothing in this ADR enters the launch binary. Every consensus part ships at an activation height of `u64::MAX` until its own ADR and a written height exist.
**Date:** 2026-11-03
**Related:** [ADR-016](ADR-016-launch-scope.md) (launch scope), [ADR-044](ADR-044-mining-rewards-lane.md) (reward lane and the `WorkClaim` kind byte), [ADR-077](ADR-077-sentinel-recovery-network.md) (AI as advisory, never executing), [ADR-078](ADR-078-omega-durable-advantage.md) (three-speed evolution), [invariants](../invariants.md)

## Context

Maya2C pays block rewards for DAG proof of work from block 1 (ADR-044). Miners never order or finalize blocks; they publish `WorkClaim`s, and the claim carries a `kind` byte that is `0` (DAG PoW) today. ADR-044 names verified AI inference as the intended second kind and says it activates at a written height, after its own ADR. This is that ADR, for a wider class of work.

Three pressures shape it:

- **Hardware is moving to rack scale.** Consumer GPUs are what the miners target today (`cuda-miner`, `wgpu-miner`, both REAL). Data-centre systems with many GPUs in one fast interconnect domain (NVIDIA NVL72-class, NVLink) are the realistic home for AI and science workloads. Their throughput is vendor-stated; nothing here has been measured on that hardware.
- **Useful compute needs verification, and the cheap verifiers are the ones that fit a chain.** Re-running a job on another node costs the same compute again. Zero-knowledge proofs are sound but expensive for dense linear algebra. Trusted hardware is fast but puts a vendor in the trust base, which collides with the existing invariant set (see "Open conflict" below).
- **Network "intelligence" is tempting and dangerous on a consensus path.** Floats, model weights and nondeterministic runtimes are exactly what invariant 20 and invariant 24 exclude. The design has to keep the chain deterministic while still letting analysis and AI help operators.

The existing building blocks are: the `WorkClaim` lane (ADR-044), staking with evidence-based slashing (ADR-028), the transparent STARK crate `crates/zk-stark` (SHIPPED, Plonky3, ADR-008), the confidential federated training crate `crates/confidential-ai` (RESEARCH, off-chain), and the activation-height mechanism used by every RESEARCH subsystem. The halo2/KZG zkML path was retired on 2026-09-21 and is not reused (invariants 20 to 23).

## Decision

### 1. Consensus stays deterministic; AI is advisory

- **No model runs on the consensus path.** Consensus rules are integer, float-free and wall-clock-free (invariants 9, 20, 24). An AI component never decides a block, a payout, a verification result or a parameter value.
- **Network intelligence is advisory.** Three permitted roles:
  1. monitoring and anomaly detection over public chain data, reporting to humans and to dashboards;
  2. parameter *proposals* (for example a fee-controller setpoint), which enter through governance and nowhere else, and become effective at an activation height that someone wrote down;
  3. operator assistance, mirroring ADR-077: an AI output can recommend, never execute.
- **Proposals are data, not code.** A proposal names a bounded integer value inside compiled limits (ADR-078 O2). It never fetches or installs a program (invariant 13).
- **Determinism is checked, not assumed.** Any state written from a job result is re-derivable by every node from the same inputs, and the state root covers it (invariant 25).

### 2. A job market for useful compute

The market is a lane on the existing reward system, not a second consensus.

- **Job spec.** A job is a content-addressed record: the workload class (inference, training step, sequence alignment, structure prediction, rendering), the input commitment, the output format, the resource bound (memory, time in heights, FLOP budget declared up front), and the verification level required. The spec is canonical bytes, hashed, and versioned.
- **Escrow.** The requester's payment is locked in a new state prefix at job creation and released by a verdict and by nothing else (agreement, an unchallenged window, or a proof). The prefix is under the state root (invariant 25), and the block that creates or settles an escrow commits in the same batch (invariant 26).
- **Payment through `WorkClaim`.** Work is claimed with `kind = 1` (verified compute). The claim references the job hash and the output commitment. Payment is a transfer from escrow, not a mint; the no-mint rule in ADR-044 is unchanged.
- **Gate (current state on master).** Kind `1` is refused at every height. The refusal is in place since #317 (commit `f7f0958b`): `BlockContext` carries `work_claim_activation`, `WORK_CLAIM_ACTIVATION_HEIGHT` is `u64::MAX`, and `crates/node/src/state/context.rs` and `settlement.rs` refuse `TxKind::ClaimWork` with `NodeError::WorkClaimInactive` before any credit or fee. Tests are in `crates/node/tests/governance_tests.rs`; ADR-044 (section "Work credit at launch") and `docs/audit/KNOWN_ISSUES.md` (#2, #18) record it. R2 lifts the refusal only through a written activation height, bound to a verified share in the same block and credited to that share's miner (ADR-044). DECISIONS.md entry B50 (2026-11-01) records the decision.

### 3. Verification ladder

Each job class declares the lowest rung that its value and risk allow. A rung is never skipped silently.

| Rung | Method | What it proves | Cost | Status in this ADR |
|---|---|---|---|---|
| (a) | Redundant execution: k workers run the same deterministic job; agreement on the output commitment; disagreement triggers a bonded re-run and slashes the minority | Majority agreement, under an honest-majority assumption per job | k times the compute | Design for R2 |
| (b) | Spot-check and optimistic challenge: one worker's result is accepted after a window measured in heights; a challenger posts a bond and a deterministic re-execution either confirms or slashes | Correct output unless someone with a bond disputes it in time | About 1 times compute plus challenge cost, if disputes are rare | Design for R3 |
| (c) | zk proof of the computation, where feasible | Correct execution, no trust in the worker | Prover cost is high; feasibility is per job class and must be measured | Research for R5, on `zk-stark` (a Plonky3 STARK, transparent and post-quantum). Invariant 22 applies: no proof verifies on chain until its parameters are real and gas is capped |
| (d) | TEE attestation of the worker | Code ran inside a vendor-attested enclave | Low | **Not decided.** Blocked until invariant 11 is reconciled (below) |

The default for any new job class is (a). Rung (b) needs a dispute window and a bond that are both expressed in heights (invariant 9 applies to freshness). Rung (c) and rung (d) are never a default.

Slashing uses the staking and evidence machinery of ADR-028. Bonds are typed and their amounts are governed values read from state (invariant 17), not constants.

### 4. Hardware: from one consumer GPU to a rack

- **Today.** A miner is one process on one GPU (`hal/cuda-miner`, `hal/wgpu-miner`). The features register records both as REAL and verified for the DAG path. No multi-GPU scheduler is recorded in `features.toml`; this ADR assumes none exists.
- **Target.** A node-local scheduler that maps a set of jobs onto a set of devices: device discovery, per-device memory budget, affinity inside one interconnect domain, and a job never split across domains unless its spec says so. The scheduler is non-consensus code. Its output is a claim, and claims are verified like any other.
- **Measurement plan.** No performance number is stated in this ADR. Every number later published carries: the git commit, the hardware (GPU model, count, interconnect, host CPU, memory), the OS and kernel, the exact command, and the raw output. It is recorded in `reports/` and says MISSING where a figure was not taken. Standing Order 2 applies: a vendor or brief figure is a target until measured.
- **Scaling claims.** Throughput, latency and efficiency claims across GPU counts are not made until a multi-device run on the target class exists. A single-node result is not extrapolated to a rack.

### 5. Data sovereignty for genetic and health data

The field-extension stance is that genetics is covered by data sovereignty: the chain does not become a store or a market for genomes.

- **Consent before compute.** A job that touches genetic, health or other personal data requires a consent record signed by the data owner, naming the workload class, the verification rung and an expiry in heights. Without it the job is refused at creation.
- **Encryption and access.** Inputs are encrypted to the owner's key (ML-KEM-based sealing, as in `crates/privacy`); the chain stores the commitment and the consent, nothing more. Decryption happens on a worker that the owner has named, and revoking consent stops new jobs, not past results.
- **No genetic data on chain.** Inputs and outputs live off chain. Commitments, consent records and verdicts are the sole on-chain data.
- **No clinical claim.** Outputs are research artefacts. Nothing here makes a medical claim, and the protocol does not sell, share or aggregate genomes.
- **Scope gate.** Compute on such data is R6, behind its own ADR and an outside review. It is not a side effect of R2 or R3.

### 6. Rust-native stack and update path

- **Rust on every node path.** Node, consensus, verifiers, schedulers and job runners are Rust. No Python, Node or other interpreter is on any node, consensus or verification path. Research notebooks may exist off the node path if they are never linked to it.
- **Updates.** Behaviour changes ship as signed releases (the node update notice of ADR-075 and the signed update channel of the Omega design, ADR-078 O1) and activate at heights written in the release. Code is never fetched from chain state.
- **Compatibility.** A new job kind is added behind its height. Nodes that do not yet have the kind refuse the claim, and the refusal is a logged rule, not a silent drop.

## Open conflict: invariant 11 and TEE attestation

This ADR does not resolve it.

- The brief for this ADR refers to invariant 11 as the TEE constraint. In `docs/invariants.md` today, **invariant 11 is "The oracle is optional and absent by default"**. No numbered invariant covers TEE attestation.
- `CLAUDE.md` (Execution Directives and the roadmap section) and `docs/architecture-vision.md` (the Maya OS row) still say that TEE attestation "collides with invariant 11". As written, those cross-references point at the oracle rule.
- Either a TEE invariant was intended and never written, or the cross-reference is stale. Both readings need a decision.
- Until it is decided: rung (d) is not used, no TEE attestation enters a consensus or payout rule, and the stale cross-references are a separate correction. Per the invariants guide, a new invariant number is written once a test pins its behaviour.

## Alternatives considered

- **AI decides consensus or payouts.** Rejected. A model's output is not reproducible across hardware, and it would put weights and float arithmetic into the state transition (invariants 20 and 24).
- **Trust a vendor enclave for all compute.** Rejected for now. It is the cheapest verifier, but it makes one vendor part of the trust base, and invariant 11 has not been reconciled with it.
- **Pay for useful work without verification.** Rejected. It turns the reward lane into a way to be paid for unchecked output.
- **A separate useful-work consensus or side chain.** Rejected. ADR-044 keeps miners out of block ordering; a second consensus would reintroduce the problem ADR-044 removes.
- **Zero-knowledge proofs for every job.** Rejected as a default. Proof cost for dense workloads is unmeasured and likely high; rung (c) stays per job class.
- **Build the market before the advisory layer.** Rejected. Advisory monitoring has no consensus effect and gives the measurements that the job-class decisions need.

## Consequences

- The chain stays deterministic and the reward lane stays the place where useful work is paid. A reader can verify every payment from the rules.
- Verification is paid for. Rung (a) doubles or triples compute cost for high-value jobs; that cost is the price of not trusting a worker, and it is stated in each job class.
- AI helps operators and governance, and it cannot move money or consensus. That is weaker than an AI that decides, and it is the reason the design can be audited.
- Rack-scale hardware is a target with no numbers yet. Until a measurement exists, the ADR promises a plan, not performance.

## What would have to be true to revisit this

- A proof system that verifies dense inference or alignment jobs at a cost the job's value can pay (rung c becomes the default).
- A written decision on invariant 11 that either accepts a named TEE vendor trust base for a job class or rules it out.
- Evidence from a multi-device run that a scheduler gains throughput over the best single-device result on the same hardware.

## Phased plan (after mainnet)

Each phase has an exit check. A phase is not done until its check passes with real command output recorded in `reports/`.

| Phase | Scope | Needs | Verification |
|---|---|---|---|
| R0 | Specs and decisions. Job spec format, `kind = 1` reserved, rung table, invariant-11 decision drafted, DECISIONS entry B50 kept current | ADR-079 accepted; owner decision on invariant 11 | Spec conformance vectors (`cargo xtask spec-coverage`); claims-check clean |
| R1 | Advisory layer. Off-chain Rust monitoring and anomaly detection over public chain data, labelled SIM where it simulates. Parameter proposals emitted as data, nothing else | R0; ADR-077 advisory rules | Replay against recorded chain history; no consensus-path change (diff check); output contains no unlabelled simulated figure |
| R2 | Escrow prefix and `WorkClaim kind 1` at `u64::MAX`. Redundant execution (rung a) with bonded slashing on a testnet | R0; staking (ADR-028); a written activation height per network | Determinism tests (same job, two machines, identical commitment); slashing tests; state-root check; testnet soak capped at 24 hours (feedback: no multi-day soaks) |
| R3 | Optimistic challenge (rung b): dispute window and bonds in heights | R2 | Adversarial tests: late challenge, double challenge, bond exhaustion, reorg during a window |
| R4 | Multi-device scheduler and the measurement harness. Single-node figures, then multi-device on the target class | R2; hardware access that the owner confirms | Each published number has commit, hardware, OS, kernel and command; MISSING where not taken |
| R5 | zk proofs (rung c) for job classes whose prover cost is measured as feasible, on `zk-stark` | R3; invariant 22 conditions met; a new ADR per proof class | Proof soundness tests with forged proofs refused; gas cap test; measured prover time and size |
| R6 | Consent-gated compute on genetic and health data | R3 at minimum; a separate ADR; outside review | Consent refusal tests; revocation tests; no genomic bytes in chain state (a state scan test) |

R0 and R1 have no consensus effect and may start on a branch as soon as mainnet is live. R2 onward needs its own activation decision.

## Explicitly not done

- No AI on the consensus, payout or verification path. No model weights or inference engine in the node.
- No performance, throughput, speed-up or TPS figure for any multi-GPU or rack-scale system. None has been measured.
- No claim that rung (d) is secure or that any TEE is trusted. The invariant-11 question is open.
- No change to issuance, fees or supply. Payments come from escrow and the existing reserve rules of ADR-044.
- No genomic or health data on chain, and no clinical or medical claim.
- No Python on any node path.
- No revival of the retired halo2/KZG zkML crates (ADR-008).
- No lattice proof-of-useful-work as a verifier; `lattice-pow` stays RESEARCH (ADR-016).
- No activation height written in this ADR. Heights are decided per network in the ADR that enables each phase.


## Amendment 2026-11-12: `WorkClaim` has no `kind` byte

Section 2 ("Payment through `WorkClaim`") and the summary line say a claim
carries `kind = 1` for verified compute. The code has no `kind` field:
`WorkClaim { beneficiary }` only (`crates/node/src/core/governance_payload.rs:120-123`),
encoded as the 32-byte beneficiary (`governance_payload.rs:168-180`). The
refusal described under "Gate" is on `TxKind::ClaimWork` as a whole
(`crates/node/src/state/settlement.rs:185-190`), not on a kind value. The
decisions in this ADR (job market as a lane on the reward system, escrow, no
mint) are unchanged. ADR-129 (branch `docs/adr-125-130-gaps`, PR #470) proposes
`TxKind::ClaimCompute` as the carrier instead of a `kind` byte on `WorkClaim`.
