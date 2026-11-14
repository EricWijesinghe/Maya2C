# Prior art: one identity and owned items across virtual worlds (avatar-passport)

**Search status:** searched 2026-11-10 (partial: two searches; OMI and ERC-6551 specs not read at source). Two searches. Open Metaverse
Interoperability Group (OMI) and ERC-6551 specifications were **not** read at
source; only secondary descriptions. This file authorizes **no** "first", "only"
or "unprecedented" claim.

## What exists today

| Project | What | Status found |
|---|---|---|
| Ready Player Me | Cross-game avatar API | Netflix announced acquisition 2025-12-19; public creator, APIs and PlayerZero ended 2026-01-31 (several secondary sources agree; they differ on whether saved avatars stay usable). Evidence that a centralized avatar layer can vanish |
| OMI Group | Open specifications for cross-platform avatar/world formats | Secondary mention only; spec text not read |
| ERC-6551 | Token-bound accounts: an NFT (ERC-721) owns a smart-contract wallet and assets | Described in secondary sources; read eips.ethereum.org before relying on it |
| W3C DID / Verifiable Credentials | Identity and credential standards | Not searched here; relevant to `did:maya2c` |

Commentary sources note no real cross-platform standard has been adopted by competing
platforms (Ready Player Me CEO interview via Decrypt).

## How a Maya2C version would differ (verified in the tree)

- Identity: `crates/identity/src/lib.rs:1-12` ("No claim preimage reaches chain state, ever. Only commitments and roots"); modules `did`, `attestation`, `document`
  (`crates/identity/src/lib.rs:53-56`). `identity-app` is a catalog row (planned, W2).
- Items: `crates/rwa` holds asset records; chain state in `crates/node/src/state/rwa.rs`.
  An NFT game exists as a WASM contract run in the real VM
  (`crates/reference-apps/tests/nft_game.rs:1-5`), with caller authorisation from
  ADR-026. That is a template, not a cross-world item standard.
- Account model: `docs/prior-art/native-smart-accounts.md` (itself only partially
  searched) is the closest analogue to ERC-6551's token-bound wallets. Whether it
  covers item-owned accounts: not verified.
- Not in the tree: any avatar format, renderer interchange, or world-side adapter.
  The hard part (game and engine adoption) is outside what a chain provides, as
  Ready Player Me's closure illustrates.

## Legal and ethical gates

- Interop standards survey must be finished first (CATALOG gate): OMI, glTF/VRM,
  ERC-6551, ERC-721, W3C VC.
- Items imported from other worlds carry third-party IP and brand licences.
- Minors' avatars and age-related claims: disclose only commitments (identity crate rule).
- No lock-in: avatar data must stay exportable in open formats.
- No speculation framing; securities and gambling law per jurisdiction.

## What may be said publicly

- Accurate: "Maya2C's identity crate keeps claim contents off chain; avatar and item interoperability are not implemented."
- **forbidden**: "first cross-world identity", "only interoperable avatar identity", "one identity for every metaverse", "unprecedented", "world's first".

## Verdict

Similar exists — differentiate by chain-anchored identity whose claims stay off
chain (`crates/identity`), not by avatar interoperability, which the tree does not
implement and which depends on third-party adoption.

## Sources (retrieved 2026-11-10; secondary)

- https://genies.com/blog/ready-player-me-shutdown
- https://avatarsdk.com/blog/2026/08/31/avatar-platforms-2026-whos-alive-whos-gone/
- https://learn.framevr.io/blog/rpm-closure
- https://www.freshconsulting.com/insights/blog/metaverse-avatar-interoperability/
- https://decrypt.co/zh-Hans/videos/interviews/TH3KEAI0/there-are-no-real-standards-ready-player-me-ceo-timmu-tke-on-building-for-an-open-gaming-ecosystem
- To read at source before use: https://eips.ethereum.org/EIPS/eip-6551

