# ADR-039: DAG-BFT quorum is n − f, not 2f + 1

**Status:** Accepted (2026-10-04) for the threshold. The committee-capture
risk recorded below is **open**: it needs Eric's decision, because every fix
changes the protocol's security or economics.
**Date:** 2026-10-04

## Context

`maya_dag_bft::Committee::quorum` returned `2f + 1` with `f = ⌊(n − 1)/3⌋`.
Certificates, round advancement and ADR-038 checkpoints all count against it.

`2f + 1` is a safe quorum only when `n = 3f + 1`. Two quorums of `q` out of
`n` share at least `2q − n` members. Safety needs that overlap to include an
honest validator, so `2q − n ≥ f + 1`:

| n | f | 2f + 1 | overlap 2q − n | safe? |
|---|---|---|---|---|
| 4 | 1 | 3 | 2 | yes |
| 5 | 1 | 3 | 1 | no: the one shared member may be the faulty one |
| 6 | 1 | 3 | 0 | no: two disjoint halves |
| 2 | 0 | 1 | 0 | no: each node certifies alone (`certify_alone`) |
| 100 | 33 | 67 | 34 | yes |

At `n = 6`, one equivocating author can propose two vertices for a round.
Each half of the committee votes for one of them, and both become
certificates. The DAG forks, and so does the committed history.

ADR-028 made this reachable. Since staking, the committee is the top
`max_validators` by stake, and its size changes whenever someone registers,
unbonds or is jailed. The deployed networks happened to sit at safe sizes:
maya-testnet-1 at `n = 1`, the four-validator rehearsals at `n = 4`. The
first registration on either would have moved it off that size.

## Decision

`quorum = n − f`.

- **Safety:** `2(n − f) − n = n − 2f ≥ f + 1` for every `n ≥ 3f + 1`.
- **Liveness:** the `n − f` honest validators can form a quorum on their own.
- **Compatibility:** at `n = 3f + 1` it equals `2f + 1`. Every committee
  that has run so far (1, 4, 100) gets the same threshold. No chain
  re-genesis, and no wire or signature change.
- **Validity** (`f + 1` votes commit an anchor) is unchanged. It needs only
  one honest voter, and `f + 1` already guarantees one.

`any_two_quorums_share_an_honest_validator_at_every_size`
(`crates/dag-bft/src/vertex.rs`) checks safety and liveness for every `n`
from 1 to 1,000. The old formula fails it at `n = 2`.

## Consequence: a cheap committee capture (open)

The threshold fix makes a second problem plain. ADR-028 already names it as
"the known gap between chosen by stake and weighted by stake".

The committee is chosen by stake, but each member has one vote. Joining
costs `min_self_bond`, which is 1,000 base units at the devnet defaults that
maya-testnet-1 uses. One faucet grant is 200,000. Whoever registers enough
keys that never come online holds more than `f` seats and stops every
quorum:

- maya-testnet-1 (`n = 1`): **one** absent registration makes `n = 2`,
  `q = 2`, and the chain halts at the next epoch boundary. Under the old
  formula it took three (`n = 4`, `q = 3`).
- Once halted, the chain cannot recover on its own. Downtime is only judged
  at an epoch boundary (`Staking::end_epoch`), and a halted chain never
  reaches one, so nothing jails the absent keys.

No shipped tool built the registration before `l1-wallet register-validator`.
The transaction itself has been valid since ADR-028.

Options, each a security or economics choice for Eric:

1. **Stake-weighted voting.** A `Committee` carrying weights, with quorums
   counted in stake rather than heads. This is the real fix, and it is what
   mainnet needs: an attacker then needs a third of the stake, not a third
   of the seats. It is a consensus change (dag-bft, certificates, ADR-038
   checkpoints), so it needs its own ADR and a re-genesis of any chain that
   adopts it.
2. **A meaningful `min_self_bond`** in each genesis. This is an economics
   parameter. It raises the price of a seat but does not remove the gap.
3. **Operator-side admission on the testnet** until (1) lands. The only
   validator refuses `Register` transactions below a bond the faucet cannot
   fund. Independent validators are funded by the operator after a
   conversation. This is node policy, not consensus: inclusion is the
   proposer's choice. It needs no re-genesis and does not restart the soak.

Recommended: (3) now on maya-testnet-1, (1) before mainnet as a launch gate,
and (2) set alongside (1) in the mainnet genesis.

## Evidence

- `cargo test -p maya-dag-bft`: lib 11 passed, including the new
  intersection test (output in the PR).
- `cargo nextest run -p custom-l1-node -p maya-dag-bft -p maya2c-node`:
  1046 tests run: 1046 passed (1 slow), 2 skipped, 83.069 s.
