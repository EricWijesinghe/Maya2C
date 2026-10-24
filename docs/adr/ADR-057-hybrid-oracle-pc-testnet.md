# ADR-057: maya-testnet-1 runs one Oracle validator and four PC validators

**Status:** Accepted (2026-10-21, Eric: "one peer in oracle and 4 on my pc").
**Date:** 2026-10-24

## Context

The testnet ran four validators on one Oracle VM, with this PC serving rpc,
status and explorer through an SSH forward. The "downtime" Eric saw was not the
VM (0 restarts, no OOM kills): the forward ran out of file handles
(`accept: Too many open files`, 171 times) and took the public services with it.

## Decision

1. **Genesis has five validators**: Oracle v1 bonded 1e9 and PC pc1..pc4 bonded
   1e8 each. Oracle holds 1e9 / 1.4e9, about 71.4%, above the 2/3 quorum, so
   the chain finalizes with every PC peer off. The three dropped Oracle bonds
   went back to their operators' allocations; total supply is unchanged
   (`make-genesis-1-plus-4.py` asserts it).
2. **Services read pc1** (RPC 8545, WebSocket P2P 31101). The one SSH forward
   left is 18601 to Oracle's RPC, used by the status page and by
   `--catch-up-from` and `--bootstrap-from`.
3. **The status page follows Oracle (18601), not a PC peer**, so a lagging peer
   cannot read as a chain halt.
4. **The watchdog heals PC peers** (`start-pc-validators.ps1`): a peer with no
   committed round for 45 min, or stopped with data older than 8 h (Oracle
   keeps 8 epochs, about 14 h), is moved aside (node key kept) and restored from
   a snapshot; a peer that is in sync but only following for 10 min is
   restarted, up to 3 times, then wiped.
5. **Oracle runs a liveness timer** (`oracle-watch.sh`): v1 is restarted when
   its committed round is flat for 3 checks.

## Consequences

- Oracle is the one chain-critical node. An Oracle outage halts the chain.
- A PC that sleeps past the jail threshold gets its peers jailed (2 epochs,
  1% bond slash) and, past 14 h, needs a snapshot restore. Gate 8
  (`docs/mainnet-v1-plan.md`) is the real fix.
- Peers started together can hang after catch-up; the watchdog now starts them
  one at a time. The cause is not found (2026-10-24).
- This is not independent decentralization (ADR-042): one VM plus one PC, run
  by one person.
