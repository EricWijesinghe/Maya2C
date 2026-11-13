# ADR-106: Sponsored fees (a paymaster co-signs to pay another account's fee)

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-4, [prior-art/sponsored-fees.md](../prior-art/sponsored-fees.md), [ADR-036](ADR-036-chain-bound-signatures.md), [ADR-029 via `crates/node/src/state/fees.rs`], [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 24, 31, 32, 33

## Context

A fee is an ordinary signed output to `FEE_COLLECTOR`, never a field
(`crates/node/src/state/fees.rs:5-11`; `required(size)` at `:92`). That is
exact and consented, and it means the sender must hold native coin before it
can do anything. Every signature commits to the chain tag, the genesis id
(ADR-036; `crates/node/src/core/transaction.rs:66`, `:299`).
`crates/smart-account/src/fees.rs:98-111` has a RESEARCH `Paymaster` with a
daily cap and budget, not wired to anything. Sui does this with a sponsor who
signs the whole transaction including gas data; Cosmos keeps a standing
allowance in state (prior-art file).

## Decision

**A sponsorship is an outer frame around an unchanged sender transaction, in
which the sponsor signs the sender's transaction id, a fee ceiling and an
expiry height under the chain tag. The fee is debited from the sponsor at
apply time; the sender's frame carries no fee output.**

### Wire

Wire version **11** (`WIRE_VERSION_SPONSORED`; 9 is PR #447, 10 is ADR-102):

```
0x0B ‖ inner_len(u32 LE) ‖ inner (any v5..v10 frame)
     ‖ Sponsorship { suite: SuiteId, sponsor_pk, max_fee: U256 (32 LE),
                     valid_until: u64, signature }
```

Sponsor signing bytes:
`"maya2c/sponsor/v1" ‖ chain_tag(32) ‖ txid(inner)(32) ‖ max_fee ‖ valid_until`.

`txid(inner)` already covers the sender's nonce, inputs, outputs and kind, so
one sponsorship fits exactly one transaction and cannot be reused once the
sender's nonce is spent. The chain tag is in the sponsor's signed bytes, so a
sponsorship signed for the testnet is worthless on mainnet (ADR-036's
property, extended to the sponsor). `valid_until` bounds how long a sponsor
is exposed to a transaction it signed.

The inner frame is byte-identical to an unsponsored one; `txid` is the inner
txid. The outer frame's own id for gossip and the block body is
`blake3(outer bytes)`, so two sponsors of the same inner transaction are two
distinct frames in the mempool and only one can apply (the nonce).

### Rules

1. The inner frame has **no** output to `FEE_COLLECTOR`; a sponsored frame
   that also pays is refused. So a relay that *adds* a sponsorship to a
   normal frame produces an invalid frame, and one that *strips* it leaves a
   frame with no fee, also refused. Neither changes what the sender signed.
2. `required(outer_len) <= max_fee`, computed over the **outer** bytes: the
   sponsor pays for its own signature bytes.
3. `height <= valid_until`.
4. The sponsor's suite is admissible at `height` (`check_admissible`,
   invariant 31) and the sponsor's address is `suite_address(suite,
   sponsor_pk)`.
5. At apply: debit `required(outer_len)` from the sponsor's balance to
   `FEE_COLLECTOR`; then apply the inner transaction as today. If the sponsor
   cannot pay, the whole frame is **refused**: the sender's nonce is not
   consumed, nothing is written. The base-fee burn and tip split are
   unchanged (`fees.rs:14-17`).
6. The sponsor's balance debit is a plain balance movement, so a sponsor that
   is a vault (invariant 32) counts it toward its window like any output to
   the collector above the 4x rule.

### Verification changes (invariant 31)

- `Transaction::from_bytes` (`transaction.rs:436-464`) gains the v11 arm and
  decodes the inner frame recursively (depth 1: an inner v11 is refused).
- `Transaction::verify` refuses a sponsored frame (no height).
- `verify_at` verifies the inner frame as today, then the sponsorship; both
  must pass. The mempool and block execution call the same function, so they
  agree (the rule invariant 31 exists for).

### DoS limits

- Mempool: at most `MAX_SPONSORED_PENDING_PER_SPONSOR` (64) frames per
  sponsor address; the 65th is dropped at admission. A sponsor's pending
  `max_fee` sum must not exceed its balance; otherwise the newest is dropped.
- A sponsored frame is evicted before an unsponsored one at the same tip,
  because its validity depends on a balance the sender does not control.
- `valid_until - height <= MAX_SPONSOR_WINDOW` (86,400 blocks) at admission,
  so a sponsor cannot be held to a year-old signature.
- Per-block: no limit beyond invariant 33; a sponsored frame is bytes like
  any other.

There is no on-chain budget or daily cap: the sponsor chooses which
transactions to sign, and the ceiling per signature is `max_fee`. A sponsor
that wants a per-user daily cap keeps that policy off chain, which is what
`smart-account::fees::Paymaster` models and may be lifted into the wallet SDK.

## Alternatives considered

- **Standing allowance record (Cosmos feegrant).** State that outlives one
  transaction, with an expiry and an undo path, and a grantee who can drain
  it up to the limit without the sponsor seeing each spend. Rejected.
- **Contract paymaster with a deposit (ERC-4337).** Needs an entry point and
  a bundler role; the chain has native accounts and no reason for either.
  Rejected.
- **A `fee_payer` field inside the sender's signed bytes (Solana).** Changes
  every sender frame and makes the sender commit to a sponsor before the
  sponsor agreed. Rejected; the outer frame leaves the sender's bytes alone.
- **Sponsor signs the inner bytes rather than the txid.** Equivalent
  binding; the txid is 32 bytes and already computed. Txid chosen.

## Consequences

- A new user can act with zero balance if an app sponsors it; the app's
  exposure is one `max_fee` per signature.
- A sponsor that lets its balance fall below pending ceilings sees those
  frames refused, never the sender's nonce burned.
- One new wire version in `from_bytes`, `verify_at`, `to_bytes`, `describe.rs`,
  the mempool's admission and eviction, and the fee apply path.
- Invariant 31's text gains "sponsored (v11)".

## Invariants touched

- **31**: v11 verifies only through `verify_at`; `verify` refuses.
- **24**: the block's `tx_root` is over outer txids; stated in `transaction_leaf`.
- **32**: a sponsor's fee debit counts toward a vault window.
- **33**: outer bytes count toward the cap.
- ADR-036: the chain tag is in the sponsor's signed bytes.

## Activation

`FeatureSet::SPONSORED_FEES = 1 << 14`, compiled
`SPONSORED_FEES_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082 with
the v4 bundle at **genesis + 15,552,000**. Before activation a v11 frame is
refused at decode time in the mempool and by `verify_at`.

## Test plan

Red tests, `crates/node/tests/sponsored_fee_tests.rs`:

- `a_v11_frame_is_refused_by_verify_and_before_activation_by_verify_at`
- `a_sponsorship_signed_for_another_chain_tag_is_refused`
- `a_sponsorship_cannot_be_reused_after_the_sender_nonce_is_spent`
- `a_sponsored_frame_that_also_pays_its_own_fee_is_refused`
- `a_stripped_sponsorship_leaves_a_frame_with_no_fee_which_is_refused`
- `a_sponsor_that_cannot_pay_voids_the_frame_without_consuming_the_nonce`
- `the_sixty_fifth_pending_frame_per_sponsor_is_dropped_at_admission`
- `a_sponsor_vault_counts_the_fee_toward_its_window`
- `base_fee_burn_and_tip_split_are_unchanged_for_a_sponsored_frame`

Vectors (`spec/02-transactions.md` SP-1..SP-4, `spec/04-fees.md` SP-5):
outer frame bytes for a v6 inner and a v7 inner; the sponsor's signing bytes;
`required(outer_len)` against `max_fee` at equality (accepted) and
`max_fee - 1` (refused).

## Open questions for Eric (economics and pricing only)

1. Should sponsored frames pay a small surcharge over `required(size)` for
   the extra admission work, or is bytes enough?
2. Are the DoS constants (64 pending per sponsor, 86,400-block window)
   acceptable, or should they be governed parameters (invariant 17)?
