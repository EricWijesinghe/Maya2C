# Prior art: Quantum-ready keys (rotation to new algorithms without user action)

**Search status:** searched 2026-11-10 (web search; sources cited below).

Feature: CATALOG §I row `quantum-ready-keys` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Partly built (suite registry, hybrid signatures, key rotation in smart accounts). "No user action" rotation is not built. R0.

**Queries:** "Algorand Falcon account key rotation post-quantum 2026"; "Ethereum post-quantum account abstraction migration EIP-7932 signature agility"; "ethereum.org roadmap quantum resistance signature agility account abstraction 2029".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Algorand | Native Falcon-1024 accounts reported live on mainnet in August 2026 (v5.0.0). Falcon-512 planned for year end 2026. Sources conflict on timing: a June roadmap put Falcon-1024 in Q3. Rekeying to Falcon authorisers described in a secondary May 2026 piece; no rotation procedure found in primary docs. | [Algorand PQ page](https://algorand.co/technology/post-quantum), [Decrypt](https://decrypt.co/371581/algorand-plans-ready-quantum-computing-threat-end-2027) |
| XRPL RegularKey | Rotate the signing key without changing the account address. Not post-quantum. | [CoinTurk](https://en.coin-turk.com/algorand-launches-live-quantum-secure-accounts-ripple-targets-2028-for-upgrade/) |
| Ethereum | Signature agility through account abstraction (EIP-8141 named on ethereum.org); target around 2029; EIP-7932 proposed as a shared signature registry; the consensus layer cannot change its cryptography without a hard fork. | [ethereum.org](https://ethereum.org/en/roadmap/future-proofing/), [EIP-7932 discussion](https://ethereum-magicians.org/t/post-quantum-migrations-crypto-agility-and-how-to-prevent-eip-7932-from-failing/27836) |
| Bitcoin BIP-360 (P2MR) | Removes the quantum-exposed key-spend path; not activated; no PQ signature BIP yet. Already cited in [pq-from-genesis.md](pq-from-genesis.md). | [delvingbitcoin](https://delvingbitcoin.org/t/changes-to-bip-360-pay-to-quantum-resistant-hash-p2qrh/1811) |

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/smart-account/src/lib.rs`: "Rotation replaces a key without changing the account id, so the address people pay stays the address."
- `crates/crypto-pq/src/suite/mod.rs`: `SuiteId` and `SuiteInfo`, the signature-suite registry (ADR-007, `docs/adr/ADR-007-signature-suites.md`).
- `docs/adr/ADR-082-genesis-committed-upgrade-levers.md`: the suite policy's deprecation schedule (`suite_deprecations`) is committed at genesis; the schedule is writable on chain after launch.
- `crates/node/src/crypto/hybrid.rs`: every transaction carries ML-DSA-65 and SLH-DSA-SHA2-128s, both must verify.

## How a Maya2C version would differ

- **Rotation keeps the account id** (XRPL does the same for classical keys). The post-quantum part is that a rotation target is a registered suite, not a fixed list.
- **A committed sunset schedule.** ADR-082 commits the suite deprecation schedule at genesis. Algorand and Ethereum describe migration as a plan; the Maya2C design commits the retirement order in the genesis-hashed state. This is a design property verified in the ADR, not a live migration record.
- **Hybrid during transition.** Both signature families must verify, so a break in one does not alone move funds. Algorand's Falcon accounts and Ethereum's plans are not described as hybrid in the sources read.

Not built: rotation without user action. Wallet UX for silent rotation is not in the repo, and "no user action" is not claimed.

**Verdict: similar exists (Algorand Falcon accounts, XRPL RegularKey, Ethereum AA signature agility). Differentiate by a genesis-committed suite sunset schedule and mandatory hybrid signatures. Silent rotation UX is unbuilt.**

**forbidden**: "first quantum-ready keys", "only chain with key rotation", "keys rotate with no user action".
