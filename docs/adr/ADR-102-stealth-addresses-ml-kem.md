# ADR-102: Stealth addresses over ML-KEM-768

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** gap row C4 ([next-evolution-gap.md](../next-evolution-gap.md)), [prior-art/stealth-addresses.md](../prior-art/stealth-addresses.md), [ADR-007](ADR-007-signature-suites.md), [ADR-013](ADR-013-suite-envelope-activation.md), [ADR-036](ADR-036-chain-bound-signatures.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), [ADR-105](ADR-105-name-service.md); invariants 24, 28, 29, 31, 33

## Context

Every output on this chain names a 32-byte address that is a hash of the
recipient's suite byte and public key (`crates/crypto-pq/src/suite/mod.rs:313-318`).
Paying the same person twice writes the same address twice, so a published
address is a public ledger of everything it ever received. Monero, ERC-5564 and
BIP-352 answer this with a one-time address per payment derived from a shared
secret (prior-art file). All three rest on elliptic-curve key homomorphism,
which FIPS 204 keys do not have, and the chain has no classical curve in
consensus.

What exists: ML-KEM-768 with the FIPS 203 sizes and an implicit-rejection
`decapsulate` (`crates/crypto-pq/src/kem.rs:63-69`, `:227`, `:253`); `sha3` as
a `crypto-pq` dependency (`Cargo.toml:89`); sealed envelopes to a KEM viewing
key in `crates/privacy/src/lib.rs:92-110` (RESEARCH); suite-tagged (v7) and
multisig (v8) frames that verify only through `verify_at`
(`crates/node/src/core/suite_tx.rs:203-225`, invariant 31). `rg -i stealth`
over `crates`, `docs`, `features.toml` finds nothing before this ADR.

## Decision

**A one-time address is a hash commitment to the recipient's spend address and
a tweak derived from an ML-KEM-768 shared secret. Receipt is unlinkable; the
spend reveals the spend address.** No new primitive: ML-KEM-768, SHAKE256 and
BLAKE3 as already used, plus the signature suites of invariant 29.

### Keys

A recipient publishes a **stealth meta-address** `(A, ek)`:

- `A` — the spend address, `suite_address(suite, spend_pk)` for any registered
  suite; the key that signs spends.
- `ek` — an ML-KEM-768 encapsulation key (1184 bytes). Its decapsulation key
  `dk` is the **scan key**: it detects payments and derives addresses, and it
  cannot spend. A wallet may hand `dk` to a scanning service, as BIP-352 does
  with its scan key.

The meta-address travels off chain or in a name record (ADR-105 `stealth`
field). It is never a transaction field.

### Sending

1. `(ct, ss) = ek.encapsulate()` (`kem.rs:227`).
2. `k = SHAKE256("maya2c/stealth/v1" ‖ ss ‖ ct)`, 64 bytes.
3. `tag = k[0]` (the view tag), `tweak = k[32..64]`.
4. `P = blake3::derive_key("maya2c/stealth/addr/v1", A ‖ tweak)`.
5. The transaction pays `P` with an ordinary `TxOutput` and carries
   `TxKind::StealthAnnounce(StealthAnnouncement { ct: [u8; 1088], tag: u8 })`.
   The announcement is a payload of the existing v6 frame; the sending wire
   format does not change. A transfer with an announcement is a payment, so
   `Module::of(StealthAnnounce)` is `None`, as for `Transfer`
   (`crates/node/src/state/invariant_guard/mod.rs:206-211`, invariant 28); it
   writes no record.

`ss` is derived per announcement and zeroized on drop (`SharedSecret`,
`kem.rs:131`); the sender keeps nothing.

### Scanning

For each announcement in a block: `ss' = dk.decapsulate(ct)` (never fails,
`kem.rs:253`), recompute `k`, compare `k[0]` with `tag`; on a match compute
`P` and look for it among the block's outputs. A wrong `dk` yields a random
`ss'`, so about 255 in 256 foreign announcements stop at the tag.

**Honest cost note.** On secp256k1 the view tag saves the second scalar
multiplication; here nothing cheaper than decapsulation depends on `ss`, so the
tag saves only the address derivation. The scanning cost is one ML-KEM-768
decapsulation per announcement on chain. The figure in
`crates/crypto-pq/src/lib.rs:31` is a handshake (785 µs release), not a
decapsulation; the per-announcement number is **MISSING** until
`cargo xtask bench` or a criterion run measures it on the implementation.

### Spending

A new authorization form, wire version **10** (`WIRE_VERSION_STEALTH`; v9 is
taken by PR #447's account-authority frame):

```
StealthAuth { suite: SuiteId, spend_pk, tweak: [u8; 32], signature }
```

`Transaction::sender()` for a v10 frame returns
`P = derive_key(A ‖ tweak)` with `A = suite_address(suite, spend_pk)`, so the
inputs it consumes must belong to `P`. The signature is over `signing_bytes`
under the chain tag (ADR-036), exactly as a v7 frame, plus the tweak, so a
relay cannot swap the tweak and redirect the proof.

Changes to verification (invariant 31, extended not weakened):

- `Transaction::verify` (`transaction.rs:360-367`) refuses v10 like v7 and v8:
  it has no height.
- `Transaction::verify_at` (`suite_tx.rs:203`) gains one arm: a `StealthAuth`
  is admissible only at or past the activation height and under
  `check_admissible(policy, suite, height)` for its suite; then
  `pq_suite::verify` under `spend_pk`.
- A frame carrying a stealth auth and any other authorization is refused, as
  for multisig (`suite_tx.rs:204-211`).

**The spend links `P` to `A`.** That is the price of hash commitment without
key homomorphism, and it is the honest difference from Monero. A recipient who
needs unlinkable spends routes the one-time output into the shielded pool once
it is on (ADR-087) or waits for the second phase below. Wallets must say so
before a user publishes a meta-address.

### Threat model

| Party | Can | Cannot |
|---|---|---|
| Sender | compute `P`; prove to a third party that it paid `P` by revealing `ss`/`ct` | spend from `P` (needs `spend_sk`) |
| Observer | see `ct`, `tag`, `P`; after a spend, see `A` | link `P` to `A` before the spend; link two unspent `P` to one recipient (both need `ss`) |
| Scanner holding `dk` | detect and derive every `P` for `A` | spend |
| Block producer | reorder or drop announcements | forge a `P` for `A` (needs `ss` for that `ct`, i.e. `dk`) |

Implicit rejection means a malformed `ct` produces no error and no oracle
(`kem.rs:245-252`). A view tag collision (1/256) costs one derivation, never a
false acceptance, because `P` is checked against the outputs. A sender who
reuses `ct` across two transactions produces the same `P` twice: allowed,
harmless, and visibly the sender's doing. Wallets refuse to *send* to a `P`
they derived more than once.

The announcement adds 1,089 bytes per payment; the base fee per byte
(`fees.rs:92`) already prices it, and invariant 33's cap bounds a block.

## Alternatives considered

- **Seed-derived one-time keys** (`from_seed(SHAKE(ss ‖ ct))`,
  `suite/validator.rs:43`): both parties compute the one-time *signing* key, so
  the sender could spend. Rejected.
- **Lattice key homomorphism** (arXiv 2501.13733): full unlinkability, but a
  custom signature key structure outside FIPS 204 and outside the suite
  registry (invariant 29). Rejected for v4; it is the R2 path if peer review
  lands.
- **Hybrid with secp256k1** (ERC-8441): adds a classical curve to consensus.
  Rejected.
- **Zero-knowledge spend** (a STARK that `P = H(A ‖ tweak)` and the signature
  verifies, without revealing `A`): removes the spend-time link. Deferred to a
  phase 2 ADR after `crates/zk-stark` has a signature-verification circuit.
- **Announcement in a separate record under a new prefix**: would make
  payments a gated module and add state for data nobody reads twice. Rejected;
  announcements live in the block body only.

## Consequences

- Payments to a meta-address are unlinkable on receipt; spends are not.
- Wallets gain a scan loop whose cost is linear in announcements per block.
  Light wallets without `dk` cannot scan; a scanning service holding `dk` is
  the delegation model.
- One new `TxKind` arm in `Module::of`, `describe.rs`, the payload codec and
  the vault allow-list (invariant 32 names the doors; `StealthAnnounce` is a
  plain transfer and must be allowed through a vault's limit like one).
- One new wire version in `from_bytes` dispatch (`transaction.rs:454-459`).
- Nothing turns on at genesis.

## Invariants touched

- **31**: `verify` refuses v10; `verify_at` is the one path. Text gains "stealth (v10)".
- **29**: the spend key is a registry suite; no new parameter set.
- **28**: `Module::of(StealthAnnounce) = None`, stated in the match.
- **24**, **33**: announcements are transaction bytes, covered by `tx_root` and the byte cap.
- **32**: the vault door list gains `StealthAnnounce` as a transfer.
- **25**: no new prefix.

## Activation

`FeatureSet::STEALTH = 1 << 10` (claimed in `adr-claims.txt`), compiled
`STEALTH_ACTIVATION_HEIGHT = u64::MAX`, switched on only by an ADR-082
`ScheduleUpgrade` under `MIN_UPGRADE_LEAD_BLOCKS`. Proposed earliest height:
**genesis + 15,552,000** (180 days at one block per second), after the v3
entries, so the v4 bundle activates together. Before activation a v10 frame is
refused by `verify_at` and never enters the mempool.

## Test plan

Red tests (fail until the feature exists), in `crates/node/tests/stealth_tests.rs`:

- `a_v10_frame_is_refused_by_verify_and_by_verify_at_before_activation`
- `a_sender_can_derive_p_but_cannot_spend_from_it`
- `the_scan_key_detects_the_payment_and_derives_the_same_p`
- `a_wrong_scan_key_matches_the_tag_at_most_by_chance_and_never_the_output`
- `a_swapped_tweak_fails_signature_verification`
- `a_stealth_announce_passes_every_breaker` (invariant 28)
- `a_vault_counts_a_stealth_payment_toward_its_limit` (invariant 32)
- `a_reorg_leaves_no_stealth_record_behind` (no state written)

Conformance vectors (`spec/02-transactions.md`, rules ST-1..ST-5, checked by
`cargo xtask spec-coverage`): fixed `ek`, `ct`, `ss` from FIPS 203 known
answers in `crates/crypto-pq/tests/acvp_tests.rs`; expected `tag`, `tweak`,
`P`; a v10 frame's bytes and txid; a negative vector with a flipped tag byte.
`crates/node/tests/suite_envelope_live_tests.rs` gains the v10 arm. A
benchmark of decapsulations per second per core, reported with commit and
hardware (Standing Order 2).

## Open questions for Eric (economics and pricing only)

1. Should an announcement pay only the per-byte base fee, or carry a fixed
   surcharge so that spam announcements cost more than their bytes?
2. Should scanning services be a paid network role (a fee for `dk`-holding
   scanners), or left entirely to wallets and apps?
