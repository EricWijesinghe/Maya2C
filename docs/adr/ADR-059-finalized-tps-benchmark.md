# ADR-059: What "TPS" means, and `cargo xtask bench-tps`

Status: Accepted (2026-10-25). Measurement tool only: no consensus change.

## Context
CLAUDE.md Production Standing Orders: "TPS" means signature-verified, executed,
state-committed, finalized transactions per second with the mix stated. No such
measurement existed (`reports/12-baseline.md` is the manual record, and the
node has had DAG-BFT finality since ADR-027). Without one, any speed claim is
unfounded and any comparison with another chain is marketing.

## Decision
`cargo xtask bench-tps` runs the four-validator localnet (real libp2p on
127.0.0.1, one process each) and reports:

- **Counted:** a transfer that was signed (ML-DSA-65 + SLH-DSA hybrid),
  accepted by a mempool, ordered by DAG-BFT, executed with its state root
  checked (invariant 24), and visible in the recipient's balance on node 3,
  which never received it. The clock runs from the first submission to the
  moment the sum of everything accepted is visible there.
- **Not counted:** signing (client work, done first in parallel), refused
  submissions (reported separately), anything not finalized when the run
  stalls (90 s without progress ends it and fails the run).
- **Mix:** single-output transfers of one base unit, N senders to one
  recipient, each sender's nonces in order. Stated in every report.
- **Disclosure in every report:** commit, OS, logical cores, binary path
  (profile), exact command, and that the machine was shared with other
  processes, which it is on the development PC.
- **Refuses debug binaries** unless `--allow-debug` (smoke runs only).

Reports are written to `reports/bench/tps-<unix time>.md` and are the only
source for a TPS figure in a document, UI or announcement
(`cargo xtask claims-check` rules on "first/fastest" still apply).

## What this does not show
Contract calls and mixed loads; a network with real latency (that needs the
stagenet containers with netem); validators on separate machines; sustained
load over hours; behaviour at 21 or 100 validators. Each is a later run of the
same tool with a different setup, recorded the same way. A number from this
tool is a lower-bound-style figure for one machine, not a claim about the
network.

## Consequences
- The first honest figure replaces "unmeasured" in CLAUDE.md and the roadmap.
- Optimisation work has a before/after to be judged by, and a regression gate
  (`scripts/bench_gate.py`) can later read the same report format.
