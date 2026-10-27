# Maya2C node-v0.1.0-testnet.5: stricter blocks, re-genesis required

> **DRAFT. Not published.** Nothing here has been tagged or released. Publishing
> needs Eric's `APPROVED: release`, and the live testnet must be re-genesised at
> the same time (see `docs/runbooks/regenesis-checklist.md`).

**Replace testnet.4, and it cannot join a testnet.4 chain.** This release changes
what a block may contain, so a testnet.5 node and a testnet.4 node disagree about
which blocks are valid. Everyone re-syncs from the new genesis. It is a testnet
release: nothing on the chain carries value.

> **Testnet, unaudited.** No external security audit has reported. Reviews so far
> are self-run. Verify every download against `SHA256SUMS`.

## What changes the chain (why a new genesis is needed)

- **Multisig spends carry exactly the required number of approvals.** A spend
  with spare approvals is refused.
- **Larger transactions are bounded and consistent:** a transaction may be up to
  4 MiB, and a consensus vertex can now carry one (before, peers refused a vertex
  holding a transaction over 1 MiB). Blocks, snapshots and undo records read their
  blobs by their own length, not a 65,536 item cap.
- **Delivery-versus-payment settlement is switched off** until the seller can
  consent (ADR-061).
- **Identity:** a revocation index too large for any page is refused instead of
  wrapping onto a low page.
- **Reorg across a channel operation** is undone correctly.
- **Staking record v3** and the other items listed in the re-genesis checklist.

## What changes only how a node behaves

- The mempool holds at most 64 MiB of transactions.
- A node that cannot read its own database while deriving a block now fails with
  an error instead of deriving an emptier block than its peers do.
- A failed vault transfer no longer blocks the valid transfer behind it when a
  node builds a block.
- Pruned nodes refuse headers at or below their horizon.

## Known and not fixed

- A tiny DEX order can stop matching on its pair (H-1), and the governance
  quorum counts work credit in turnout but not in the eligible total (H-2).
  Both are in `reports/sessions/2026-10-26-audit-findings.md`.
- Finalized throughput has one measurement from one machine (see `reports/bench/`);
  it is not a network figure.
