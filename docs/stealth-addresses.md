# Stealth addresses over ML-KEM-768

**Status:** RESEARCH, dark. `STEALTH_ACTIVATION_HEIGHT = u64::MAX`
(`crates/node/src/state/context.rs`); an upgrade entry naming
`FeatureSet::STEALTH` (bit 10) is refused. Decision: [ADR-102](adr/ADR-102-stealth-addresses-ml-kem.md).
Prior art: [prior-art/stealth-addresses.md](prior-art/stealth-addresses.md).
Code: `crates/crypto-pq/src/stealth.rs`. Spec rules: ST-1..ST-5 in
[spec/02-transactions.md](../spec/02-transactions.md).

## What it does

A recipient publishes one **meta-address**; every payment to it lands on a
fresh **one-time address**. An observer cannot tell that two such payments went
to the same person, or to the published address, until one of them is spent.

## Keys

| Key | Who holds it | Can |
|---|---|---|
| Spend key (`spend_sk`, any registry suite) | the recipient only | spend |
| Scan seed (64 bytes, FIPS 203 `d ‖ z`) → **view key** | the recipient, or a scanning service | find payments and their addresses; **cannot spend** |
| Meta-address `A ‖ ek` (1216 bytes) | public | lets anyone pay |

`A` is the ordinary suite address of `spend_pk`; `ek` is the ML-KEM-768
encapsulation key of the view key. `ScanSeed` and `ViewKey` are zeroized on
drop and print as `<redacted>`.

## Sending

1. `(ct, ss) = ML-KEM-768.Encaps(ek)`.
2. `k = SHAKE256("maya2c/stealth/v1" ‖ ss ‖ ct)`, 64 bytes; `tag = k[0]`,
   `tweak = k[32..64]`.
3. `P = BLAKE3-derive-key("maya2c/stealth/addr/v1", A ‖ tweak)`.
4. Pay `P`, and publish the **announcement** `ct ‖ tag` (1089 bytes).

The sender keeps nothing: `ss` and the tweak are dropped and wiped.

## Scanning

For each announcement: decapsulate `ct` with the view key, recompute `k`,
compare the tag; on a match compute `P` and look for it among that
transaction's outputs. Only an output equal to `P` is a detection, so the 1 in
256 tag collisions cost one extra hash and never a false payment.

**Cost.** ML-KEM has nothing cheaper than decapsulation that depends on the
shared secret, so every announcement costs one decapsulation — the view tag
only skips the address derivation. Measured with
`cargo bench -p maya-crypto-pq --bench stealth_scan`, one core:

| Population (2,000 announcements each) | µs per announcement | per second |
|---|---|---|
| foreign (another recipient's) | 28.89 | 34,616 |
| own (detected) | 29.23 | 34,211 |

Measured 2026-11-17 on branch `feat/v3-stealth` (base `14bbd2f8`, the
commit that adds this page), Intel Core Ultra 9 275HX, 31 GB RAM, Windows 11
Pro Insider build 29708, `rustc 1.99.0-nightly (da80ed070 2026-07-14)`, bench
profile, one core, while other builds shared the machine. One run, no
confidence interval. The two rows are within a percent of each other because
the decapsulation dominates both; the tag saves only the derivation.

## Spending

A spend presents `StealthAuth { suite, spend_pk, tweak, signature }`, the
signature over `"maya2c/stealth/spend/v1" ‖ tweak ‖ message`. The verifier
recomputes `P` from `suite_address(suite, spend_pk)` and the tweak. Changing
the tweak breaks the signature; another key authorizes another `P`.

**The spend reveals `A`.** FIPS 204 keys cannot be tweaked the way elliptic
curve keys can, so there is no one-time secret key: spending shows which
meta-address `P` belonged to. Receipt is private; spending is not. A wallet must
say so before a user publishes a meta-address. Unlinkable spends need the
shielded pool or a zero-knowledge spend (ADR-102, "Alternatives").

## Not built yet

- The `StealthAnnounce` transaction kind and the wire-v10 spend frame (ADR-102
  §Spending), with `verify_at`, `Module::of`, the vault door list and the
  `crates/node/tests/stealth_tests.rs` red tests the ADR lists.
- Wallet UI and the ADR-105 name-record `stealth` field.
- External cryptographic review.
- Eric's open questions in ADR-102 (announcement surcharge, paid scanning
  services).
