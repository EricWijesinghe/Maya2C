# Prior art: Shielded transfers of user-issued tokens

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: "Zcash ZIP 226 227 Zcash Shielded Assets design asset base note commitment"; "Solana Token-2022 confidential transfer extension ElGamal auditor"). Zcash sources are draft-era ZIP mirrors; the merged text may differ.

Feature: parity row P-20, [ADR-110](../adr/ADR-110-shielded-user-tokens.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Zcash Shielded Assets (ZIP 226 transfer/burn, ZIP 227 issuance) | A ZSA note is an Orchard note plus an Asset Base derived from the asset digest by hash-to-curve; commitments go into the same Orchard tree; the circuit checks the same base in old notes, new notes and the value commitment; the ZEC case is computed without a base so native commitments are unchanged; issuance is transparent at first. Classical (Pallas curve, Sinsemilla). | [ZIP 226 draft](https://www.github.com/QED-it/zips/blob/81b6a00126024acfded51584bc5c7ab39132a654/zip-0226.rst), [ZIP 227 draft](https://gitea.interbiznw.com/zec/zips/blame/commit/3651df5ffc17efca3dd86d25b70dc898596cfb3b/zip-0227.rst) |
| Solana Token-2022 confidential transfers | Encrypted balances under twisted ElGamal with sigma protocols and Bulletproof range proofs; an optional global auditor key on the mint decrypts every amount; the extension must be set at mint creation. Classical. Audited by Least Authority. | [Solana issuer guide](https://solana.com/docs/tokens/extensions/confidential-transfer/issuer-guide), [Least Authority audit](https://leastauthority.com/blog/audit-of-anza-technology-confidential-transfers-in-solana-token-2022-program/) |

## What Maya2C has (checked in repo, 2026-11-12)

- A native-only STARK shielded pool: `ShieldedPool { tree, anchors, balance }` (`crates/node/src/state/shielded.rs:55-67`), `Note { value: u64, address, rho, rand }` (`crates/zk-stark/src/pool/note.rs:81-90`), 2-in-2-out AIR over `BabyBear`, value at most `2^58 - 1` (ADR-067).
- User-issued assets with `U256` supply (`crates/node/src/state/asset.rs`; ADR-103 adds mint authority and cap).
- `CIRCUIT_IS_AUDITED = false` (`features.toml`, shielded-pool-stark-joinsplit); mainnet v1 ships with the pool off (ADR-037).

## How the Maya2C version differs

- **Post-quantum**: the proof is a hash-based STARK (Poseidon2 inside, Keccak outside) with no setup; both surveyed designs rest on elliptic curves.
- **Asset in the clear, balance per asset in the clear**: the asset id is a public input and the pool keeps one auditable balance per asset. Zcash hides the asset behind the value-commitment base; Maya2C does not, on purpose, to keep every token's hidden supply auditable from outside.
- **One tree, one nullifier set** for every asset, as ZSA; native notes keep byte-identical commitments, as ZSA's ZEC case.
- **No auditor key on the asset**: viewing-key work is separate (`features.toml` pq-viewing-keys, RESEARCH).

**Verdict: similar exists (ZSA drafts, Token-2022). Differentiate by the post-quantum proof system and the clear per-asset pool balance. No "first" claim.**
