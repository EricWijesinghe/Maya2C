# Prior art: proving a person is a person without biometrics on chain (human-or-machine)

**Search status:** searched 2026-11-10 (partial: one comparison search; no primary protocol documents fetched). One comparison search; results
were largely vendor/consultancy comparison pages (chainscorelabs.com, human.tech),
whose user counts conflict and are not relied on. No primary protocol documents
were fetched. This file authorizes **no** "first", "only" or "unprecedented" claim.

## What exists today

| Project | Method | Trade-off reported |
|---|---|---|
| World ID (Worldcoin) | Iris scan on proprietary Orb hardware; WLD token | Strong sybil resistance at scale; hardware-dependent; reported banned or restricted in several countries over privacy |
| BrightID | Social-graph vouching, phone only | Cheaper, more private; weaker to collusion |
| Idena | Synchronous "flip" puzzle sessions about every two weeks; iDNA paid to validators | Free to join; cognitive test; cost of recurring sessions |
| Proof of Humanity | Video submission, vouching, refundable deposit | On-chain registry; public video |

User counts: sources disagree (World ID 4.5M to 38M); none relied on.

## How a Maya2C version would differ (verified in the tree)

- `crates/personhood/src/lib.rs:1-12`: the biometric template never leaves the
  device. A fuzzy commitment (Juels-Wattenberg repetition code) binds a 128-bit
  secret that seeds an ML-DSA-65 key; only the public key is published.
  World ID's reported design also keeps the iris image off chain, so "no biometric
  on chain" alone is not a difference; the post-quantum signature key is the
  concrete one.
- What it **lacks** (`crates/personhood/src/lib.rs:14-24`, `features.toml:2353-2361`):
  no liveness (needs TEE attestation, none exists), no uniqueness across
  enrollments (so it does not stop one person enrolling twice, which is the core
  sybil problem the projects above address), no iris extractor; tests use
  synthetic templates.
- Class `RESEARCH`, status `working`; 100,000 verifications measured at 5% bit
  noise with 3 false rejects (`features.toml:2361`, synthetic data).
- Repetition-code helper data leaks template information; it must never be
  stored off device (`crates/personhood/src/lib.rs:23-24`).

So today the crate proves "the enrolled device holder is present", not "this is a
unique human". It does not compete with the systems above on their problem.

## Legal and ethical gates

- Biometric data is special-category under GDPR and regulated in Illinois (BIPA)
  and elsewhere; the privacy red team (CATALOG gate) must precede any pilot.
- Worldcoin-style bans are the cautionary precedent: opt-in only, no inducement
  payments, deletion path, no biometric uniqueness registry without counsel.
- Exclusion harm: people without usable irises, or without smartphones, must have
  a non-biometric route.
- Must not gate consensus or voting until uniqueness is solved (`crates/personhood` is RESEARCH).

## What may be said publicly

- Accurate: "Maya2C's personhood crate checks that an enrolled device holder is present, under a post-quantum key; it does not prove uniqueness."
- **forbidden**: "first proof of personhood", "only biometric-free proof of humanity", "sybil-proof", "unprecedented", "world's first".

## Verdict

Similar exists — differentiate by on-device-only fuzzy commitment feeding a
post-quantum key, but uniqueness and liveness are unbuilt, so no sybil-resistance
claim is supportable.

## Sources (retrieved 2026-11-10; secondary, vendor-influenced)

- https://passport.human.tech/blog/proof-of-personhood-explained-how-it-works-who-s-building-it-and-why-it-matters-now
- https://poy-verify.pages.dev/compare/best-worldcoin-alternatives-2026
- https://www.chainscorelabs.com/comparisons/identity-soulbound-vs-verifiable-credentials/proof-of-personhood-systems/worldcoin-vs-brightid-centralized-biometric-vs-decentralized-social-graph
- https://www.chainscorelabs.com/comparisons/dao-governance-on-chain-vs-off-chain/sybil-resistance-and-identity/brightid-vs-idena

