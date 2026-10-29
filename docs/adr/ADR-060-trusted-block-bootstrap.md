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
`--trusted-block HEIGHT:HEX` (with `--bootstrap-from`; alone it is a startup
error, not silently ignored) names one block the operator vouches for from a
**second path**. The bootstrap then refuses the source unless:
1. the headers it served form a chain by height (each header's parent is the
   id before it; previously only "each parent is known" was checked, so a
   repeated or skipped header could pass with the right count);
2. its chain has exactly that id at that height. Height 0 (the genesis everyone
   has) and heights above its tip never match;
3. the trusted height is **at or above the snapshot's height**. Block ids commit
   to their ancestors, so the anchor vouches only for the chain up to its own
   height; a snapshot above it sits on headers it says nothing about;
4. every body fetched above the snapshot extends the verified header chain, and
   the tip's id equals the verified one.
Any failure purges the database like any failed bootstrap. Without the flag the
node prints "no --trusted-block: the source alone decides which chain this node
starts on".

This is the weak-subjectivity checkpoint of proof-of-stake chains. It is not a
proof: it is only as good as the second source, it says nothing about work
(there is almost none to compare), and it covers the chain choice, not a
snapshot taken on the right chain (the state root covers that).

## Use
- Operators: take `height` and `block id` from two paths that are not the
  bootstrap source (a node you run via `get_checkpoint`, and the explorer), and
  choose the **newest** block available: it must be at or above the snapshot the
  source will serve, and an old anchor protects only the chain below it.
- The PC watchdog should pass the newest checkpoint of a *local voting peer*
  when restoring from Oracle, so the Oracle path and the local path must agree.
  Wiring that is a change to the live script and needs a restart cycle; it is
  proposed, not done.

## Not done
- A committee-signed checkpoint check inside bootstrap (needs the epoch's
  committee, which is the state being restored: the usual chicken-and-egg).
- Making the flag mandatory on mainnet. It is a **launch gate**, not a
  suggestion; that is Eric's decision, and it is not made here.
- Retrying another source after a refusal (a lagging source below the anchor is
  refused, which is correct and fail-closed).

## Review
Self-run security review (pre-audit, not independent) found the first version
weaker than this ADR then claimed: no ordering rule between anchor and snapshot,
no header linkage check, bodies above the snapshot unbound, a silently ignored
flag. Each is fixed with a failing test first (`pruned_node_tests`: 17 pass).
