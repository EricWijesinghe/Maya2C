---
title: 'Re-sealing archives when algorithms age'
editUrl: false
# GENERATED from docs/resealing.md by scripts/ingest.mjs. Edit the source, not this.
---
An archive of historical blocks is read long after it is written. Its
integrity rests on two primitives, and either can age:

| Layer | Primitive | Where |
|---|---|---|
| **Content address** | BLAKE3-256 multihash (`0x1e`) in every CID — the section bytes and the manifest | `crates/archive/src/car.rs` |
| **Seal** | SLH-DSA-SHAKE-256f over `domain ‖ epoch ‖ root CID`, one forward-secure key per epoch | `crates/archive/src/seal.rs` |

The rule this plan follows: **never replace evidence, add to it.** A re-seal
wraps the old seal rather than discarding it, so each layer of the record
stays checkable. A verifier who distrusts the new algorithm can still check
the old one, for whatever it is still worth.

## What counts as aging

Act on any of the following. The column says how urgent it is.

| Signal | Example | Urgency |
|---|---|---|
| NIST deprecation schedule for a parameter set | SP 800-131A-style transition dates for SLH-DSA-SHAKE-256f | Plan: reseal before the disallow date |
| A published attack that lowers a parameter set's security below its category | a structural result against SHAKE256 or BLAKE3 | Reseal within one release cycle |
| A practical second-preimage or forgery | a demonstrated SLH-DSA forgery, or a BLAKE3 second preimage | Emergency: reseal now, and treat seals made after the attack date as suspect |
| The chain's own crypto-agility policy retiring the suite | `SuitePolicy` deprecation window for `0x21` (`crates/crypto-pq/src/agility/`) | Follow the policy's window |

A BLAKE3 *collision* is not a trigger for the content layer. An attacker
who can produce collisions still cannot make an existing archive's bytes
hash to a different CID. It **is** a trigger for anything sealed *after* the
attack became feasible, since such a batch could have been built with a
twin.

## Procedure A: the seal algorithm ages

Content addresses are still sound; only SLH-DSA-SHAKE-256f is in doubt.

1. **Choose the successor suite** in an ADR. It must be a registry suite
   (`crates/crypto-pq/src/suite/`) with NIST vectors in
   `tests/acvp_tests.rs`, and not in the same family as the one being
   retired unless the break is parameter-specific.
2. **Start a new key chain** under that suite: a fresh genesis key,
   published out of band exactly as the first one was. Do not certify it
   from the old chain. Once the old algorithm is in doubt, a certificate it
   signed is worth no more than the old algorithm.
3. **Re-seal every archive root.** The new seal signs
   `domain_v2 ‖ epoch' ‖ root CID ‖ old seal bytes`, which binds the new
   attestation to the old one rather than standing beside it. The old seal
   stays in place.
4. **Publish both chains' transitions** alongside the archives. A verifier
   checks the newest seal it trusts and may also check the older ones.
5. **Record the cut-over epoch** in the archive index. From that point,
   seals under the old suite alone are refused for new archives.

Work: one signature per archive root, never a pass over the archive bytes.
At SLH-DSA-SHAKE-256f's size, that is about 49 KB of new signature per
archive.

## Procedure B: the content hash ages

BLAKE3 is in doubt, so CIDs are too.

1. **Choose the successor multihash** in an ADR — SHA3-256 is the natural
   one: already in the tree, and a different construction (a sponge, not
   BLAKE3's ChaCha-derived compression).
   `car.rs` refuses every multihash but BLAKE3-256 today, so this step
   includes teaching the reader the new code.
2. **Re-address every archive.** Read it, verify it under the old CIDs
   *first*, re-encode it with CIDs under the new multihash, and write it as
   a new archive. The block bytes are unchanged; only their names change.
3. **Seal the new root with a cross-reference:** `domain ‖ epoch ‖ new root
   ‖ old root`. That records that the sealer checked the old archive and
   that the new one is its re-addressing.
4. **Keep the old archives** until every consumer reads the new roots.
   Deleting them destroys the evidence that step 2 was honest.

Work: one full read and write of every archive, plus one seal each. Budget
it like a re-sync.

## Procedure C: a sealer key is compromised

This is not an algorithm aging, but the key chain is built for it. Forward
security means epochs **before** the compromise stay sound: their keys were
erased, and the seed chain is one-way (`seal.rs`, pinned by
`a_compromise_at_epoch_50_cannot_forge_the_past`).

1. **Announce the compromise epoch `c`.** Verifiers must refuse every seal
   and transition at or after `c` from that chain.
2. **Start a new chain** under the same suite, with a new genesis key
   published out of band.
3. **Re-seal the archives sealed at or after `c`.** Archives sealed before
   `c` need nothing.

## What is not handled here

- Detection. Nothing in the tree watches for published attacks. The
  triggers above are a human decision, recorded in an ADR.
- Revocation distribution. Today a compromise announcement travels out of
  band. A transparency log for transitions and revocations is the obvious
  next step, and is not built.
