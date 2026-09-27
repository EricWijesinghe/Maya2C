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
| Which approved deposit funds came from | everyone | a STARK proof that a deposit is in an approved set's tree, bound to its nullifier (`maya-zk-stark::association`) |
| A contract's private field (e.g. a balance) | everyone, including the contract's other users | the field is a commitment; updates carry a STARK proof (`maya-zk-stark::private_state`, `maya_priv` in the VM) |

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
- **The amount of a private-field update.** The field's value is hidden;
  each debit or credit amount is public, and so is which contract and when.
  A field that is only ever credited once and debited once leaks its value.
- **Private fields are capped at 2^58 base units,** and they are not on chain
  yet: `maya_priv` is opt-in in the VM, not in the consensus import surface,
  and needs an audit and an activation height first.
- **The approved set itself,** and whether its provider approved the right
  deposits — as with the flagged list.
- **Whoever holds your viewing key sees everything you sealed to it.** A
  per-transaction disclosure key shows one transaction; the account key shows
  all of them.

## Proof sizes and cost

Measured 2026-09-28 on the Windows workstation (the proof crate at
`opt-level = 3` with debug assertions on, so the prover also checks its own constraints):

| proof | prove | verify | size |
|---|---|---|---|
| private-field debit (`private_state`) | 12–133 ms | (in the contract call) | 236,946 bytes; the contract call costs 1,468,144 gas |
| association-set inclusion, depth 15 | 16–103 ms | 7–9 ms | 345,625 bytes |

Ranges are across runs: the first proof in a process pays one-time setup.

The non-membership proof over 1,000 flagged entries is **731,521 bytes**.
It proves in 72 ms and verifies in 47 ms on a 4-vCPU desktop with debug
assertions on (`reports/27-privacy.md`). Neither a phone nor an optimized
build was measured. At that size, a proof per transaction costs more block
space than the post-quantum signature.
