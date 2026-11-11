# ADR-128: Threshold decryption for contracts — a second committee key, height-conditioned reveals delivered by timer, the sealed-mempool key left alone

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled. **Changes the protocol's trust model (a second committee); flagged under "When to stop and ask Eric".**
**Date:** 2026-11-12
**Related:** parity row P-41, [prior-art/threshold-decryption.md](../prior-art/threshold-decryption.md), [ADR-025](ADR-025-mev-position.md), ADR-114 (contract timers, branch `docs/adr-110-115-gaps`), [ADR-127](ADR-127-vrf-host-function.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 11, 21 (retired, binds host fuel), 24, 25, 28, 33

## Context

The sealed mempool is `crates/mev` plus `crates/node/src/sealed/` and
`state/sealed_exec.rs` (core, REAL, verified; `features.toml` row
`threshold-encrypted-mempool`). Its shape:

- A `t`-of-`n` committee with one aggregate ristretto255 key
  (`mev/src/committee.rs:20-36`); `Committee::generate` holds the whole
  key for one function call — "the setup is trusted", DKG not built
  (`mev/src/lib.rs:45-49`).
- A sender seals a `TxKind` to the committee key with an ephemeral
  ElGamal-style point and ChaCha20-Poly1305, bound to associated data
  (`cipher.rs:115-140`); the envelope record stores sender and ciphertext
  under `m:env:<reveal height BE><id>` (`sealed/mod.rs:98-121`).
- A member's share is `ephemeral · s_i` with a Chaum–Pedersen proof
  (`cipher.rs:250-262`), **per ciphertext**: a share opens one envelope
  and leaks nothing about another. Members are told to compute a share
  "after the block fixing the payload's position has been committed,
  never before" (`cipher.rs:236-241`). Every node verifies every share
  (`MAX_REVEAL_SHARES_PER_BLOCK = 1_024`, `sealed/mod.rs:57-62`).
- `settle_sealed` opens the envelopes due at a height in key order and
  executes the inner kind; short of `threshold` shares the envelope
  expires and nothing happens (`sealed_exec.rs:294-345`, `:405-440`).
  Reveal delay is bounded: `MIN_REVEAL_DELAY = 2`, `MAX_REVEAL_DELAY =
  256` (`sealed/mod.rs:64-77`).
- The committee is installed at genesis and never rotates
  (`sealed/mod.rs:79-86`); it is optional (`genesis.rs:80-92`; invariant
  11's shape).
- The crate's own justification for a classical curve on a post-quantum
  chain is that "the confidentiality here has a lifetime of roughly one
  block" (`mev/src/lib.rs:56-66`).

**Findings that contradict the brief.** The parity row says "reuse the
sealed-mempool committee". Reusing the *key* is cryptographically
harmless per ciphertext (shares are per ephemeral point), but three
properties of that committee do not carry over to a contract primitive:

1. **The lifetime argument.** A contract that holds a ciphertext for an
   auction closing in 10,000 blocks has a secret with a 10,000-block
   lifetime, and the "roughly one block" exception that justifies
   ristretto255 no longer holds. A contract primitive must either keep
   the same cap (`MAX_REVEAL_DELAY`) or use a post-quantum scheme that
   does not exist as a standard (prior-art file).
2. **The member's trigger rule.** Members produce shares for envelopes
   whose position is committed, at their reveal height. A contract
   request is a different trigger with a different condition (a height
   named by a contract, after a state condition); mixing two trigger
   rules on one committee is how a member is talked into revealing a
   mempool envelope early.
3. **The share budget.** `MAX_REVEAL_SHARES_PER_BLOCK` bounds the
   verification work a reveal block does; contract reveals compete for
   it. A second budget on a second record keeps the mempool's liveness
   independent of contract demand.

So: same crate, same cryptography, **separate committee record, separate
budget, same delay cap**.

## Decision

**A contract calls `threshold_seal_request(ciphertext_ptr, len,
reveal_height, callback_input…) -> i64` with a ciphertext sealed off-chain
to the *contract committee's* key (`m:cc:` record, distinct from
`m:committee`). The request is stored under `m:cr:<reveal height
BE><id>` next to the mempool's envelopes. At `reveal_height` the end-of-
block pass opens it with the members' shares, exactly as `settle_sealed`
does, and delivers the plaintext to the contract through a timer-style
callback (ADR-114). Short of shares, the request expires and the callback
receives "expired". The reveal height is bounded by the same
`MAX_REVEAL_DELAY = 256`, so the classical-exception argument of
`mev/src/lib.rs` holds for contract data too.**

### Records (invariant 25)

Under the existing `m:` prefix and `StateLayer` (one scan already
collects the subsystem, `sealed/mod.rs:9-14`):

- `m:cc:` → `CommitteeRecord` (same type, `sealed/mod.rs:87-95`), the
  contract committee. Installed at genesis **or** by ADR-082's upgrade
  schedule at the feature height, since this chain's genesis has passed
  by then; absent by default.
- `m:cr:` ‖ `reveal_height(8 BE)` ‖ `id(32)` → `ContractRequest {
  contract: Address, ciphertext: Vec<u8>, callback_input: Vec<u8>,
  gas_limit: u64 }`. `id = blake3("maya2c threshold request v1" ‖
  contract ‖ txid ‖ index)`.
- `m:cs:` ‖ `reveal_height` ‖ `id` ‖ `member(2)` → `DecryptionShare`
  (the mempool's share record shape, `sealed/mod.rs:124`), accepted by a
  `RevealShare`-style kind that names a request id; `MAX_CONTRACT_SHARES_PER_BLOCK
  = 512`, a separate budget.
- Escrow for the callback's fee: ADR-114's `ct:e:contract` is reused;
  `threshold_seal_request` returns a negative code when it cannot cover
  `TIMER_FEE`, never a trap.

### Host functions

| Function | Rule |
|---|---|
| `threshold_committee_key(out_ptr) -> i32` | Writes the 32-byte `m:cc:` encryption key; `-1` when no contract committee is installed (invariant 11's shape: a chain without one answers "absent", not an error). |
| `threshold_seal_request(ct_ptr, ct_len, reveal_height, input_ptr, input_len, gas_limit) -> i64` | `height + MIN_REVEAL_DELAY <= reveal_height <= height + MAX_REVEAL_DELAY`; `ct_len <= MAX_SEALED_CIPHERTEXT` (`core/sealed_payload.rs`); the ciphertext's associated data must be `id ‖ reveal_height` (checked at open, as `envelope_aad` is, `sealed_exec.rs:420`), so a ciphertext cannot be replayed under another request. Charges fuel for the record write first (invariant 21). At most `MAX_REQUESTS_PER_CALL = 4`. Returns the request index or a negative code. |
| callback `on_threshold_reveal(id, status, plaintext)` | Delivered by the ADR-114 timer runner at `reveal_height`, after `settle_sealed`, in `m:cr:` key order; `status` is `0` opened / `1` expired. Runs on a copy committed on success, as revealed envelopes do (`sealed_exec.rs:327-333`); a failing callback is the contract's loss and the block continues. |

The plaintext is a byte string the contract interprets; the host never
decodes it as a `TxKind`, which is the difference from the mempool path
and the reason nesting (`sealed_exec.rs:432-438`) is not a concern here.

### What members do

A contract-committee member watches `m:cr:` for requests whose reveal
height is the next block, computes `decryption_share` on the request's
`SealedPayload` and submits it. The trigger is "the request's height has
arrived", a pure function of committed state, so members never decide
*whether* to reveal — the chain's rule does, which is what keeps a member
from being a party to the contract's outcome. Members who reveal early
off-chain cannot be stopped by any scheme (the mempool crate says the same,
`cipher.rs:236-241`); the committee's composition is the trust decision.

### What this is not

- Not FHE, not MPC over the plaintext (Fhenix, Arcium in the prior-art
  file): the contract sees the plaintext after the height, in the clear,
  on every node. The property is **nobody sees it before the height**.
- Not conditional on anything but height. Fairblock-style "decrypt when
  price > X" needs a member to evaluate a state condition off-chain; the
  contract achieves it by not scheduling the request until the condition
  holds, which keeps members condition-blind.
- Not post-quantum. Same bounded exception as the mempool, same cap, and
  the same sentence: when a standardized PQ threshold scheme exists, this
  is the record to replace (a second `scheme` byte in `m:cc:` is reserved
  for it; refused until then, the ADR-115 pattern).
- Not DKG. `Committee::generate` is trusted setup; a contract committee
  is generated the same way and says so where the crate does
  (`mev/src/lib.rs:45-49`). The `docs/sealed-mempool.md` the crate points
  to does not exist (ADR-025 line 36 records the dangling link); writing
  it is part of this ADR's PR.

## Alternatives considered

- **Reuse `m:committee` with a request flag.** Findings 1–3; rejected.
- **Timelock / delay encryption (VDF-based).** No post-quantum VDF; no
  committee is the attraction, but a VDF puts a wall-clock-ish
  assumption on hardware speed, and invariant 9's reasoning (height, not
  time) argues against it.
- **Witness encryption.** Research (Fairblock's stated direction); no
  implementation to vet.
- **TEE decryption.** Invariant 11's unresolved TEE question (ADR-079,
  "Open conflict").
- **Deliver plaintext as a `CallContract` the opener submits.** The opener
  is then an unprivileged party who can withhold; the timer runner is
  the chain, which cannot.

## Consequences

- One committee record, two request/share records, three host-side
  entry points, one new share-submission kind, one callback through
  ADR-114's runner. All under `m:`.
- A second trust set exists, disclosed like the first. The chain runs
  without it; contracts calling `threshold_committee_key` get `-1`.
- Block work: up to 512 share proofs (two scalar multiplications each)
  plus the callbacks' gas, bounded by ADR-114's per-block timer cap.
- A contract author gets sealed-bid auctions, hidden moves and
  commit-free reveals with a 256-block ceiling, stated.

## Invariants touched

- **11**: the contract committee is optional and absent by default; a
  genesis or schedule without it writes no `m:cc:` record.
- **21** (retired): fuel for the record write and the share verification
  budget, charged first.
- **24**: open-or-expire at a height is a pure function of committed
  shares; every node computes the same callback input.
- **25**: three records under `m:`; undo covered by the existing `m:`
  journal section.
- **28**: `Module::Sealed`'s breaker covers the contract path too: a
  tripped breaker admits no request and opens none (requests then expire
  at their height, which is the safe direction).
- **33**: ciphertext size capped by `MAX_SEALED_CIPHERTEXT`.

## Activation

`FeatureSet::THRESHOLD_DECRYPT = 1 << 34` (claimed in `adr-claims.txt`),
compiled `THRESHOLD_DECRYPT_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**, with ADR-114. The
committee record arrives in the same scheduled upgrade (a scheduled
`m:cc:` install, the way ADR-082 schedules a suite sunset), or not at
all. Before the height every host function here returns "absent" and the
share kind is refused.

## Test plan

Red tests, `crates/node/tests/threshold_contract_tests.rs` and
`crates/vm/tests/threshold_host_tests.rs`:

- `a_request_opens_at_its_height_and_the_callback_sees_the_plaintext`
- `a_request_short_of_threshold_shares_expires_and_the_callback_sees_expired`
- `a_ciphertext_whose_aad_names_another_request_does_not_open`
- `a_reveal_height_outside_min_and_max_delay_is_refused_at_request_time`
- `a_mempool_share_cannot_be_submitted_against_a_contract_request_and_vice_versa`
- `contract_shares_do_not_consume_the_mempool_share_budget`
- `a_failing_callback_leaves_state_as_before_and_the_block_continues`
- `a_tripped_sealed_breaker_admits_no_request`
- `without_a_contract_committee_every_host_function_answers_absent`
- `a_reorg_restores_requests_and_shares`
- `exploit_replays::a_member_who_reveals_early_cannot_change_the_order_or_the_height`

Vectors: `spec/` sealed-mempool section gains TD-1..TD-3 (a request, a
share set that opens it, a share set one short).

## Open questions for Eric (economics only)

1. Whether contract-committee members are paid per share (today mempool
   members are not), and from what: a request fee the contract escrows
   (`TIMER_FEE`-style) is the shape if yes.
2. The request fee itself, if any, beyond ADR-114's timer fee.

The trust-model question (a second committee, classical curve under a
256-block cap) is not economics and is flagged in the status line.
