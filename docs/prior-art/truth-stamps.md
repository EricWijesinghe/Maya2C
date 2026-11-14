# Prior art: proving a photo or video is original since capture (truth-stamps)

**Search status:** searched 2026-11-10 (partial: two searches; C2PA spec and ERC-7053 not read at source). Two searches. C2PA specification
text, the C2PA trust list and Numbers Protocol's ERC-7053 were not read at
source; several results were low-authority blogs and disagree on dates. Whether
C2PA has any post-quantum algorithm profile was **not confirmed**. This file
authorizes **no** "first", "only" or "unprecedented" claim.

## What exists today

| Project | What | Notes |
|---|---|---|
| C2PA / Content Credentials | Signed manifests recording provenance and edit history; founded 2021 by Adobe, Arm, BBC, Intel, Microsoft, Truepic | Open standard; metadata can be stripped on re-share |
| Truepic | Mobile SDKs that sign at capture; C2PA-trusted certificate authority for enterprises | Commercial |
| Leica M11-P (2023) | Camera with a security chip holding signing keys | Dates differ across blogs |
| Google Pixel 10 | Signs photos by default with Titan M2 keys and an on-device timestamp authority | One secondary source |
| Nikon Z6 III | C2PA credentials suspended and certificates revoked after a signing vulnerability | Capture-side key theft is real |
| Numbers Protocol | Capture app adds C2PA and registers metadata on its own chain under ERC-7053 | Company materials only |
| pqc-content-provenance (PyPI) | Third-party PQC C2PA-style package for AI output | Not part of the C2PA spec; unreviewed |
| IETF RFC 9882 | ML-DSA in CMS | PKI standard, not a C2PA commitment |

## How a Maya2C version would differ (verified in the tree)

- Anchoring a hash on a post-quantum chain is the `notary` row (hash only, never
  the file). Numbers Protocol already anchors provenance on a chain, so chain
  anchoring is not a difference.
- Post-quantum signing at capture is the row's stated candidate difference. Repo
  support: ML-DSA-65 and SLH-DSA in `crates/crypto-pq`; forward-secure SLH-DSA
  seals for archives (`crates/archive/src/seal.rs:1-30`). A signing path inside a
  camera or phone is **not** in the tree.
- The weak link is capture hardware: `attested-hardware` is gated on a TEE
  invariant that does not exist yet (same gap blocks liveness in
  `crates/personhood/src/lib.rs:14-17`). Without it a stamp proves "someone with
  this key signed these bytes", which is the problem Nikon's revocation shows.
- Compatibility with C2PA is open; if post-quantum algorithms are not accepted in
  the C2PA trust model, a stamp would sit beside C2PA, not inside it. Not verified.

## Legal and ethical gates

- "Verified original" must not be read as "true": a genuine camera can photograph a
  staged scene. Wording needs counsel and a UX that says what is proven.
- Privacy: stamps carry location, time and device identity; opt-in, strip-able, no
  device-linkable public keys by default.
- No key extraction; vulnerability disclosure for capture keys.
- Liability for false positives or negatives in disputes, courts and elections.
- Trust governance: avoid a Maya2C-only trust list that excludes honest cameras.

## What may be said publicly

- Accurate: "Maya2C has no capture-side stamp today; its post-quantum signing primitives are in `crates/crypto-pq`."
- **forbidden**: "first post-quantum photo provenance", "only quantum-safe content credentials", "tamper-proof photos", "unprecedented", "world's first".

## Verdict

Similar exists, widely deployed — differentiate by post-quantum signing at capture
and chain anchoring of the stamp, both unbuilt and dependent on an attestation
invariant that does not exist yet. No claimable difference today.

## Sources (retrieved 2026-11-10; several secondary)

- https://en.wikipedia.org/wiki/Content_Credentials
- https://truepic.com/c2pa/signing
- https://c2paviewer.com/articles/leica-m11-p-c2pa
- https://www.softwareseni.com/how-c2pa-content-credentials-work-and-what-their-limits-are/
- https://docs.captureapp.xyz/c2pa
- https://iptc.org/download/events/phmdc2024/Numbers_Protocol_EIP-7053-IPTC-PMD-2024.pdf
- https://pypi.org/project/pqc-content-provenance/
- https://www.rfc-editor.org/rfc/rfc9882.html
