# ADR-069: post-mainnet feature program (eleven features)

- Status: Proposed (2026-10-29, Eric: "start working on everything suggested").
- Scope: after the mainnet code freeze (2026-11-03). Nothing here enters the launch binary.

## Decision

Eleven features (BACKLOG.md P5, 2026-10-29) are built after mainnet, each on its own branch,
in this order, and each switched on by an activation height that ships at `u64::MAX` until its
own ADR, tests, spec rules and vectors are in (the house pattern, ADR-002):

| # | Feature | Layer | Builds on |
|---|---|---|---|
| 1 | Duress keys | consensus (account rule) + wallet | multi-key accounts, suite tags |
| 2 | Safety-window payments | consensus (escrow record) + wallet | state records, `threat-intel` |
| 3 | Decentralization-aware rewards | consensus (reward split) | `staking/concentration.rs`, ADR-068 |
| 4 | Cryptographic sunset schedule | consensus (suite policy) | `crypto-pq/agility`, invariant 29 |
| 5 | Living inheritance | consensus (time-locked rule) | time locks, multisig |
| 6 | PQ time-locked messages/payments | consensus + client | sealed mempool, ML-KEM |
| 7 | Phone-mined first fees | consensus (share lane) + apps | ADR-044 share lane |
| 8 | On-chain AI judge | consensus (deterministic inference) | fee-market `MODEL_V1` |
| 9 | AI-work mining | consensus (WorkClaim kind) | ADR-044 |
| 10 | Quantum Harbor safe-deposit | consensus record + interop | `quantum-harbor` |
| 11 | Offline payments | clients + relays | chat DTN design |

Scam Shield (1, 2) goes first: highest user impact, smallest new surface.

## Rules that apply to every one

- A dated `docs/prior-art/<feature>.md` search before any "first" claim (Standing Order 10).
- Consensus changes get spec rules, conformance vectors, and rust-reviewer + security-reviewer.
- No change to the launch binary before mainnet; no third parties (Eric, 2026-10-28).
- Phone mining stays visible and opt-in (platform-compliance rule).
