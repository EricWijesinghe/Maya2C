# The Maya2C specification

**Version 0.1.0** (2026-09-26) — covers the rules a node enforces **today**.
Where the target design differs (DAG-BFT finality, staking), the section says
so and lists the rule as a gap instead of specifying behaviour no code has.

| § | File | Owner | Version | Vectors |
|---|---|---|---|---|
| 1 | [01-encoding.md](01-encoding.md) | protocol | 0.1.0 | `tests/encoding.json` |
| 2 | [02-transactions.md](02-transactions.md) | protocol | 0.1.0 | `tests/encoding.json`, `tests/state_transitions.json` |
| 3 | [03-state.md](03-state.md) | protocol | 0.1.0 | `tests/state_transitions.json` |
| 4 | [04-fees.md](04-fees.md) | economics | 0.1.0 | `tests/fees.json` |
| 5 | [05-consensus.md](05-consensus.md) | consensus | 0.1.0 | none yet — every rule is a listed gap |
| 6 | [06-staking.md](06-staking.md) | consensus | 0.0.0 | not built |
| 7 | [07-networking.md](07-networking.md) | networking | 0.1.0 | none (not consensus) |
| 8 | [08-crypto.md](08-crypto.md) | cryptography | 0.1.0 | FIPS KATs in `crates/crypto-pq/tests/` |

"Owner" is a role, not a person: nobody is assigned yet. A change to any
section goes through a [MIP](mips/README.md) and bumps that section's
version.

## Rules

Every rule has an ID, written `- **ID** …` at the start of a line. IDs are
never reused. A rule marked *(positive only: …)* has no input that it
rejects — it defines a value rather than refusing one — so it needs no
negative vector. `cargo xtask spec-coverage` lists every rule and whether a
vector in `tests/` exercises it for acceptance and for rejection.

## How the vectors are made and checked

```
spec/tests/keys.json          node-produced: 4 hybrid keys from fixed seeds
        │
        ▼
crates/spec-ref  ──►  spec/tests/{state_transitions,encoding,fees}.json
(reference, no node code)          │
                                   ├─► crates/node/tests/conformance.rs   (production node)
                                   └─► spec/verifier-ts/verify.ts          (TypeScript, own BLAKE3)
```

- `cargo run -p maya-spec-ref --bin spec-vectors` regenerates; `-- --check`
  fails if the committed vectors are stale.
- `cargo test -p custom-l1-node --test conformance` replays them against
  the node, with real signatures.
- `node spec/verifier-ts/verify.ts` replays them independently. It trusts
  each vector's signature flag and says so.
