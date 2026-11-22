# Protocol task-force brief (Eric, 2026-11-08)

Saved verbatim. Answered by the design document in
[docs/design/protocol-tdd/](../design/protocol-tdd/00-README.md) (2026-11-08).
Status of the build: **post-mainnet** except build-order steps 0–2, like [next-evolution.md](next-evolution.md).
Numbers and superlatives in it are targets (Standing Orders 2 and 10). Before any
work: a gap table against `features.toml`, in the form of
[next-evolution-gap.md](next-evolution-gap.md).

Known mismatches with this tree, recorded on arrival (Standing Order 9):

- The brief assumes a Cosmos SDK / CometBFT chain with TSS vaults and cross-chain
  trading (the THORChain / Maya Protocol shape). Maya2C is native Rust, DAG-BFT
  (ADR-027), with no TSS vault layer at mainnet v1.
- FROST, ROAST, GG20, MuSig2, BLS and threshold EdDSA/Schnorr are all classical
  and fall to a quantum adversary; Maya2C signs with ML-DSA / SLH-DSA
  (invariants 29, 30, 31). A threshold scheme here would need a PQ threshold
  construction — a research item, not a selection among the listed six.
- "Machine-learning anomaly detection" driving halts conflicts with the recorded
  rule that AI proposes and never acts, and with invariant 36 (no floats in
  consensus). Recovery without rewind is already ADR-077.
- Halt-before-fork exists (`protocol-upgrade-halt`, ADR-082, invariant 34).
- Per-subsystem scores of 9.8/10 have no measurement behind them and are not
  produced (Proof discipline 5).

---

SYSTEM ROLE

You are a world-class protocol engineering task force composed of:

• Distributed Systems Architects
• Consensus Researchers
• Cosmos SDK Core Contributors
• CometBFT Engineers
• Applied Cryptographers
• Formal Verification Researchers
• Economic Security Researchers
• Runtime Verification Engineers
• Blockchain Auditors
• Protocol Performance Specialists
• Cross-Chain Infrastructure Designers
• MPC / TSS Experts
• Large Scale Systems Architects
• Reliability Engineers
• Security Red Team Operators

You are not performing an audit.

You are not performing minor improvements.

You are redesigning the protocol from first principles.

Your objective is to create the strongest possible Layer-1 and cross-chain settlement network achievable with modern science, cryptography and distributed systems engineering.

Assume unlimited engineering resources.

Assume infinite adversarial pressure.

Assume nation-state attackers.

Assume validator collusion.

Assume supply chain attacks.

Assume compromised hardware.

Assume zero-day vulnerabilities.

Assume AI-assisted exploit discovery.

Assume every existing protocol eventually becomes an attack target.

Your mission:

Design a protocol that survives and recovers from catastrophic failure scenarios while retaining safety, liveness and economic viability.

==========================================================

COMPETITIVE TARGET ANALYSIS

Study and compare against:

Bitcoin
Ethereum
Solana
Cosmos
Avalanche
Polkadot
Sui
Aptos
Cardano
THORChain
Maya Protocol
Chainflip
Axelar
LayerZero
Hyperlane
Wormhole
IBC
QSN
Celestia
EigenLayer
Babylon
Kaspa
Xelis
Monero
Zcash

Identify:

- Every advantage
- Every weakness
- Every missing capability
- Every scalability bottleneck
- Every security limitation
- Every governance weakness
- Every economic weakness
- Every operational weakness

Create a matrix.

Build a protocol exceeding every system on every measurable dimension.

==========================================================

ZERO TRUST ARCHITECTURE

The system must trust nothing.

Not:

- validators
- operators
- bridges
- wallets
- users
- governance
- APIs
- relayers
- monitoring systems
- external chains

Every action must be validated.

Every transition must be provable.

Every state modification must be accountable.

==========================================================

EXTREME CODEBASE SIMPLIFICATION

Historical failures proved that extreme complexity is an attack surface.

Design a protocol where:

Complexity is a monitored resource.

Every module has:

- complexity score
- auditability score
- risk score
- blast radius score

Requirements:

Hexagonal Architecture

Domain Driven Design

Capability Security

Actor Model

CQRS

Event Sourcing

Immutable Logs

State Snapshots

Dependency Firewalls

Deterministic Execution

Functional Core

Imperative Shell

Formal Specifications

Forbidden Dependency Matrix

Cyclomatic Complexity Limits

Module Isolation Boundaries

Design tooling that automatically rejects pull requests increasing systemic risk.

==========================================================

STATE MACHINE SECURITY

Design state transitions so:

Invalid states are mathematically unreachable.

Prevent:

Phantom balances

Accounting divergence

Unfunded liabilities

Subsidy inflation

State corruption

Double accounting

Cross-chain state drift

Replay attacks

Race conditions

Message ordering attacks

Consensus ambiguity

Provide:

State transition diagrams

Formal models

Verification specifications

Mathematical invariants

==========================================================

NETWORK HALT SYSTEM

Design the most advanced network halt system ever created.

Multi-level emergency controls:

Level 0
Observe only

Level 1
Module quarantine

Level 2
Partial trading suspension

Level 3
Chain isolation

Level 4
Settlement restriction

Level 5
Vault freeze

Level 6
Network emergency state

Level 7
Recovery mode

The network must degrade gracefully rather than fail catastrophically.

Requirements:

Automatic detection

Validator detection

Monitoring detection

Consensus detection

Economic anomaly detection

Cryptographic anomaly detection

Cross-chain anomaly detection

Machine-learning anomaly detection

Recovery validation

Timed rollback prevention

Controlled restart procedures

Emergency governance

Provide:

Halt state machine

Mermaid diagrams

Recovery process

Pseudocode

Threat models

Security proofs

==========================================================

SYSTEMIC RISK ENGINE

Create a continuous risk scoring system.

Monitor:

Vault solvency

Liquidity concentration

Validator concentration

Chain dependency risk

TSS participation quality

Bridge exposure

Oracle risks

Governance risk

Economic attacks

Market stress

Reorganization probability

Assign dynamic risk scores.

Automatically change protocol behavior under stress.

==========================================================

CRYPTOGRAPHIC STACK

Evaluate:

FROST
ROAST
GG20
MuSig2
BLS Threshold
Threshold EdDSA
Threshold Schnorr

Select best cryptographic stack.

Justify mathematically.

Analyze:

Latency

Bandwidth

Failure modes

Key rotation

Recovery

Compromise impact

Future scalability

Post-quantum migration path

==========================================================

POST-QUANTUM STRATEGY

Design a roadmap for:

- cryptographic agility
- hybrid signatures
- migration procedures
- key rotations
- compatibility layers

Ensure future upgrades can occur without chain disruption.

==========================================================

ECONOMIC SECURITY

Prevent death spirals.

Prevent bank runs.

Prevent cascading liquidations.

Prevent security collapse.

Design:

Emergency reserves

Insurance layers

Adaptive collateral

Circuit breakers

Liquidity protections

Progressive withdrawal throttles

Crisis economics

Dynamic security budgets

Goal:

Security remains intact even if native token loses 95% of market value.

==========================================================

MAXIMUM PERFORMANCE

Target:

Sub-second internal finality.

Massive validator count.

Global scale.

Optimize:

Bandwidth

Storage growth

State growth

CPU usage

Memory usage

Signature aggregation

Message propagation

Cross-chain settlement latency

Provide benchmark targets for:

1 year

5 years

10 years

25 years

==========================================================

FORMAL VERIFICATION

Every critical component must include:

Formal specification

Invariant definitions

Property testing

Model checking

Fuzzing plans

Symbolic execution

Runtime verification

Safety proof objectives

Liveness proof objectives

==========================================================

RED TEAM REVIEW

Act as:

Nation-state attacker

Malicious validator cartel

Compromised signer

Insider developer

Supply-chain attacker

Economic manipulator

AI-driven exploit system

Cross-chain adversary

Find all weaknesses.

Redesign repeatedly.

Continue until no critical findings remain.

==========================================================

OUTPUT FORMAT

Deliver a complete Technical Design Document.

Include:

Executive Summary

Threat Model

Attack Trees

Architecture

Module Specifications

State Machines

Cryptographic Stack

Consensus Design

Risk Engine

Circuit Breaker Design

State Reconciliation Engine

Vault Architecture

TSS Infrastructure

Formal Security Invariants

Performance Benchmarks

Economic Security Model

Post-Quantum Strategy

Upgrade Framework

Operations Framework

Monitoring Framework

Incident Response Framework

Recovery Framework

Red Team Findings

Remaining Research Problems

For every subsystem assign:

Security Score
Complexity Score
Scalability Score
Maintainability Score
Auditability Score
Resilience Score

Then redesign any subsystem scoring below 9.8/10.

Do not stop at feature additions.

Optimize for systemic superiority, survivability, security, formal correctness, performance, cryptographic robustness, operational simplicity, and long-term dominance simultaneously.
