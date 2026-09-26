# What privacy on Maya2C hides, and what it does not

Master Prompt 27 §5. Plain language.

**Today: the shielded pool is dark.** Its circuit has not been audited
(`CIRCUIT_IS_AUDITED = false`, ADR-016), so no network uses it. Everything
below describes the design and the pieces that exist.

## Hidden, when the pool is active

| What | From whom | How |
|---|---|---|
| Amounts and counterparties of shielded transfers | everyone without a viewing key | notes and a STARK proof (`maya-zk-stark::pool`); details sealed in envelopes (`crates/privacy`) |
| Which deposit a withdrawal came from | everyone | the joinsplit proves membership without saying which note |
| Whether funds come from a flagged set | nobody: this is proven, not hidden | a STARK non-membership proof against a committed list (`maya-zk-stark::sanctions`) |

## Not hidden

- **Transparent transfers.** Every normal transaction is public: sender,
  recipients, amounts.
- **Timing.** When a shielded transaction lands is public. A deposit and a
  withdrawal close in time, with a matching amount, can be linked.
- **Amounts on public paths.** Moving into or out of the pool is a
  transparent transaction, amount included.
- **Network metadata.** The p2p layer does not hide your IP address from the
  peers you send to. Use your own node, or a network-level anonymizer.
- **The flagged list itself.** A non-membership proof hides *which*
  identifier you are, not the list, and it proves nothing about whether the
  list is right.
- **Whoever holds your viewing key sees everything you sealed to it.** A
  per-transaction disclosure key shows one transaction; the account key shows
  all of them.

## Proof sizes and cost

The non-membership proof over 1,000 flagged entries is **731,521 bytes**.
It proves in 72 ms and verifies in 47 ms on a 4-vCPU desktop with debug
assertions on (`reports/27-privacy.md`). Neither a phone nor an optimized
build was measured. At that size, a proof per transaction costs more block
space than the post-quantum signature.
