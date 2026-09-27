# Cross-chain messages — draft v0.1

Master Prompt 25 §1, §4. No connection is live. This page holds the trust
table the brief asks to publish, and the message shape.

## Trust assumptions per connection

| Connection | What an attacker must break | Status |
|---|---|---|
| Bitcoin → Maya2C | Bitcoin's proof of work beyond the confirmation depth: an attacker must out-mine Bitcoin for that many blocks (`crates/btc-spv`: compact targets, retarget, chainwork, reorgs; real mainnet headers 0–2 in tests) | header verification built; no value moves |
| Ethereum → Maya2C | two thirds of Ethereum's current 512-member sync committee (BLS12-381), plus SHA-256 and Keccak-256. `interop::beacon` verifies the committee against a trusted bootstrap root, the aggregate signature over the attested header, and Merkle branches down to the finalized execution block hash, which `interop::eth` checks against the full header (real mainnet fixtures in tests). **The trusted bootstrap root is an assumption** (weak subjectivity), committee handover across periods is not built, BLS falls to a quantum computer, and no STARK wraps any of it | finality light client built and tested on mainnet data; no value moves |
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
