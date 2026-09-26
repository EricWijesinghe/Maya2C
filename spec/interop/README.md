# Cross-chain messages — draft v0.1

Master Prompt 25 §1, §4. No connection is live. This page holds the trust
table the brief asks to publish, and the message shape.

## Trust assumptions per connection

| Connection | What an attacker must break | Status |
|---|---|---|
| Bitcoin → Maya2C | Bitcoin's proof of work beyond the confirmation depth: an attacker must out-mine Bitcoin for that many blocks (`crates/btc-spv`: compact targets, retarget, chainwork, reorgs; real mainnet headers 0–2 in tests) | header verification built; no value moves |
| Ethereum → Maya2C | today, only Keccak-256 and RLP. Headers hash-check and link (`crates/interop::eth`, verified on the real mainnet genesis header), but **finality is not verified**. Sync-committee signatures are BLS12-381, which a quantum computer breaks, and no STARK wraps them | header hashing only; **not a light client of finality** |
| Solana, Cosmos | not built | — |
| Any route | a bug in the route's own code loses at most the route's current cap (`routes::Route`: starts small, grows with clean days, an incident resets it) | built, tested |

No connection depends on a multisig of operators. That is the rule, and
nothing live exists to break it.

## Message shape

```
version      u8        = 1
source       u32       chain id (registry to be written)
destination  u32
nonce        u64       per (source, destination) route
payload      bytes     application message
proof_ref    bytes32   commitment to the source-chain proof (header hash + inclusion path root)
signature    hybrid ML-DSA-65 + SLH-DSA over all of the above (optional for proof-verified routes)
```

A message is accepted only against a verified proof. The signature
identifies a relayer; it is never the reason a message is believed.
