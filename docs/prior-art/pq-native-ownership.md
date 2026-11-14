# Prior art: Native tokens and NFTs on a post-quantum chain

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: v4 P-1 and P-2, [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-1, §P-2, design text ADR-103/104 (PR #465), which carry [fungible-tokens.md](fungible-tokens.md) and [nfts.md](nfts.md) for ERC-20/SPL/ERC-721 comparison. This file covers the PQ wording.

**Queries:** "post-quantum NFT token standard quantum-resistant chain native tokens mint cap QRL Algorand Falcon".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| QRL (Quantum Resistant Ledger) | XMSS-signed chain live since 2018; announced preliminary native "quantum secure NFTs" on devnet in 2021 and sought a genesis artist launch; no formal token standard or supply cap found in the post. | [QRL blog](https://www.theqrl.org/blog/announcing-preliminary-quantum-secure-nfts-and-cta/) |
| Algorand | Chose Falcon for state proofs; native ASAs (tokens and NFTs) with a fixed total and a manager/reserve/freeze/clawback address set exist on classical keys. | [algorand.co on Falcon](https://algorand.co/learn/what-is-falcon-a-nist-endorsed-digital-signature-algorithm-for-quantum-security) |
| Autheo | Vendor FAQ says THEO token transfers are signed with NIST PQ algorithms including Falcon. Unverified. | [autheo.com FAQ](https://www.autheo.com/faq/how-does-autheos-post-quantum-security-protect-token-holders) |
| General | Overview of "quantum-resistant tokens" as a category. | [yellow.com](https://yellow.com/learn/what-are-quantum-resistant-tokens-how-they-secure-crypto-from-quantum-computing-threats) |

## What Maya2C has (checked in repo, 2026-11-12)

- `RegisterAsset`/`TransferAsset` with a fixed supply, DEX pools over
  assets, no mint by decision (`crates/node/src/state/asset.rs:17-25`).
- No unique-asset record.
- Every signature is a registry PQ suite from genesis.

## How the Maya2C version differs

- Tokens: an immutable cap and a renounceable authority on the existing
  record, no decimals, no freeze or clawback (Algorand ASAs have both; a
  freeze authority is a dilution-of-rights the chain does not offer).
- NFTs: a 32-byte metadata hash per item, no on-chain blob, no royalty rule
  in v4; an owner index for wallets.
- Both are PQ-signed because every transaction is; QRL's XMSS NFTs are the
  closest precedent.

**Verdict: similar exists (QRL native NFTs; Algorand ASAs). Differentiate
by cap-and-renounce semantics and hash-only items. No "first" claim.**
