# ADR-060: A trusted block anchors snapshot bootstrap

Status: Accepted (2026-10-25). Node-side check only: no consensus, state or wire change.

## Context
The self-run topology review (`reports/security/2026-10-25-hybrid-topology-threat-model.md`,
finding 4, confirmed in `state_pruner/snapshot.rs`) found that `--bootstrap-from`
trusts its source for which chain it is on. The bootstrap checks that headers
link and meet the proof-of-work target, that the snapshot sits on that header
chain, and that it reproduces the header's state root. On a DAG-BFT chain the
target is near zero (`difficulty_bits: 0` on the testnet), so a lying source
(Oracle through the SSH forward, `bootstrap.maya2c.dev` for joiners, or anything
on the path) can serve a self-consistent chain that no committee signed.
Catch-up afterwards would refuse to link to the attested checkpoint, so the
harm is a node (and the RPC and wallets reading it) on a fake chain, not a fork.

## Decision
`--trusted-block HEIGHT:HEX` (with `--bootstrap-from`) names one block the
operator vouches for from a **second path**. After the headers are synced and
before any snapshot is fetched, the bootstrap refuses the source unless its
best chain has exactly that id at that height (or the height is above its tip),
and purges as for any failed bootstrap. Without the flag the node says so:
"no --trusted-block: the source alone decides which chain this node starts on".

This is the weak-subjectivity checkpoint of proof-of-stake chains. It is not a
proof: it is only as good as the second source, and it covers the chain choice,
not a snapshot taken on the right chain (that is covered by the state root).

## Use
- Operators: take `height` and `block id` from a node you run (`get_checkpoint`
  or `get_block_by_height`) or from the explorer, not from the bootstrap source.
- The PC watchdog should pass the newest checkpoint of a *local voting peer*
  when restoring from Oracle, so the Oracle path and the local path must agree.
  Wiring that is a change to the live script and needs a restart cycle; it is
  proposed, not done.

## Not done
- A committee-signed checkpoint check inside bootstrap (needs the epoch's
  committee, which is the state being restored: the usual chicken-and-egg).
- Making the flag mandatory on mainnet. It should be; that is a launch decision.
