# ADR-127: A VRF host function for contracts — `vrf_verify` with measured fuel, and the commit-then-draw pattern on timers

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-40, [prior-art/contract-randomness.md](../prior-art/contract-randomness.md), ADR-115 (post-quantum VRF, branch `docs/adr-110-115-gaps`), ADR-114 (contract timers, same branch), [ADR-024](ADR-024-oracle-choice.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 10, 11, 21 (retired, still binds host fuel), 24

## Context

Contracts already read randomness. `block_randomness(out_ptr) -> i32`
(`crates/vm/src/runtime.rs:635`, `:853-873`) writes `RANDOMNESS_LEN = 32`
bytes (`host.rs:54`) of the **previous block's beacon** and returns `-1`
when the chain has no beacon, which is the default (invariant 11: no
oracle, no `o:` records, no beacon; `state/contracts.rs:180` hands the VM
an `Option`). The beacon is `beacon_n = H(previous ‖ VRF(sk_proposer,
previous ‖ height))` (`crates/node/src/oracle/beacon.rs:5-9`), ECVRF per
RFC 9381 from `crates/vrf` (`ecvrf.rs:136`, `:189`; suite octet `0x03`,
invariant 10), and a miner has "one bit of influence over each block it
mines" by including or omitting the proof (`beacon.rs:27`). ADR-115 adds a
BLAKE3 hash-chain scheme (tag 2) mixed into the same beacon and reserves
tag 3 for a lattice VRF behind a research gate.

**Findings that contradict the brief.**

1. The parity row says P-40 "exposes `features.toml:1893`" — the
   `crates/vrf` subsystem — as if contracts had no randomness. They have
   the beacon through `block_randomness` since the host list was closed
   (`runtime.rs:625-650`). What they do **not** have is (a) a way to
   verify a VRF proof of their own choosing (Algorand's `vrf_verify`
   opcode, prior-art file), so a contract cannot run its own lottery
   over a key it registered; and (b) a safe way to draw a *future* value
   that the caller could not have seen when committing, which is the
   flaw every "read the previous block" scheme has (prior-art: Sui and
   Aptos make randomness-reading entry points private so a wrapper
   cannot revert on a losing draw).
2. `block_randomness` returns the *previous* block's value: a transaction
   author sees it before sending. For a one-transaction game that is a
   known bias, not a bug; it is why (b) matters.
3. The beacon depends on the oracle registry being configured. On a chain
   without one, every randomness host function returns "absent", and
   this ADR does not change that.

## Decision

**Two things. (1) A host function `vrf_verify(scheme: i32, pk_ptr,
alpha_ptr, alpha_len, proof_ptr, proof_len, out_ptr) -> i32` that verifies
a VRF proof under a scheme tag `crates/vrf` knows and writes the 64-byte
output, charging a measured fuel price before it reads a byte. (2) No new
randomness source: the commit-then-draw pattern is documented as a
composition of `schedule_timer` (ADR-114) and `block_randomness`, and the
VM gains one rule that makes it safe — a call that reads
`block_randomness` is refused if it is nested inside another contract's
call.**

### `vrf_verify`

- `scheme`: `VrfScheme::tag()` (`crates/vrf/src/scheme.rs:48`): `1` ECVRF
  (RFC 9381, `ecvrf::verify`, `ecvrf.rs:189`), `2` hash-chain once ADR-115
  lands (its verify needs the chain's committed index, so for scheme 2 the
  "proof" is the reveal and the public key is `r_0 ‖ index`; the contract
  tracks the index it last accepted), `3` refused until ADR-115's gate.
  An unknown tag returns `-2`; the decoder refuses it the way
  `from_tag` does, so every node agrees.
- Lengths are fixed by the scheme: `PUBLIC_KEY_LEN = 32`, `PROOF_LEN =
  80` for ECVRF (`ecvrf.rs:75`), `OUTPUT_LEN = 64` (`scheme.rs:34`).
  `alpha_len <= MAX_VRF_ALPHA = 256`.
- Returns `64` and writes the output on success, `-1` on an invalid
  proof, `-2` unknown scheme, `-3` bad lengths. Never traps on a
  verification failure: a contract decides what a bad proof means.
- **Fuel first** (invariant 21's pattern, `crates/vm/src/zkml.rs:59-75`):
  `VRF_VERIFY_FUEL[scheme] + VRF_ALPHA_FUEL_PER_BYTE * alpha_len`, charged
  before the pointers are read, with the constants calibrated by
  `crates/vm/tests/fuel_calibration_tests.rs` against measured ECVRF
  verification on this hardware and recorded in `reports/`. No number is
  stated here.
- Added to `HOST_FUNCTIONS` (`runtime.rs:625`) under the feature height:
  before the height the import resolves and returns `-2` for every
  scheme, so a contract compiled against it validates on every node
  (the rule the `host_verify_zkml_proof` comment states, `runtime.rs:643-648`).

What it enables: a contract registers a VRF public key for an operator
(a game, a raffle, an oracle of its own), the operator submits `(alpha,
proof)`, and the contract derives its draw from the verified output. The
contract chooses `alpha` to be something the operator could not pick:
the beacon at a height, or a hash of commitments already in storage.

### Commit-then-draw, as a pattern, with one VM rule

A contract that needs a value nobody could know at commit time:

1. On commit, store the entries and call `schedule_timer(height + D, …)`
   (ADR-114) with `D >= 2`.
2. In the timer callback, read `block_randomness` — the beacon of the
   block before the timer ran, which was unknown when the entries were
   committed — and settle.

The beacon's producer bias is the miner's one bit per block
(`beacon.rs:27`) composed over `D` blocks; the contract documents `D` as
its security parameter. This ADR adds **no** new randomness source
because none is stronger than the beacon ADR-115 already strengthens.

The VM rule: **`block_randomness` returns `-4` when the current call is a
nested call from another contract.** A wrapper that calls a lottery
contract and reverts on a loss is the prior-art failure mode; with the
rule, a randomness-reading entry point is reachable from a transaction or
a timer and from nothing else. This is the Sui/Aptos "private entry"
restriction, enforced by the host rather than by a compiler the VM does
not have. Timer callbacks are top-level by construction (ADR-114 runs
them at block end).

### What this is not

- Not a new beacon. The beacon is ADR-115's.
- Not an oracle. A VRF key a contract registers is that contract's trust
  decision, not the chain's.
- Not available without an oracle registry (`block_randomness` stays
  absent, `vrf_verify` works regardless because it verifies what the
  caller supplies).

## Alternatives considered

- **A `request_randomness` / `fulfill` host pair (Chainlink VRF's
  shape).** Needs a fulfiller role and a fee path; the timer plus beacon
  gives the same future-value property with machinery that already has
  an ADR.
- **Expose the proposer's VRF secret-derived output for a caller-chosen
  alpha.** The node does not hold a VRF key on behalf of contracts and
  must not (Standing Order 5).
- **A per-call cryptographic RNG seeded by `txid`.** Known to the sender;
  rejected.
- **A drand-style external beacon (Sui).** An external committee of
  BLS signers is classical and a trusted party; invariant 11 exists to
  make such a party a written decision, and ADR-024 chose the oracle
  path.
- **Do nothing.** Contracts cannot verify a proof today; a raffle
  operator's output is taken on trust.

## Consequences

- One host function, one VM rule, two fuel constants calibrated with
  real output, one addition to the closed import list.
- `crates/vm` gains a dependency on `crates/vrf` (core, already a node
  dependency; no new external crate — ADR-005).
- A nested call to a contract that reads randomness fails with `-4`; a
  contract author who wanted composition writes a timer instead.
- Scheme 2 verification through this host is stateful on the contract's
  side (it keeps the last index); the ADR says so rather than pretending
  a hash-chain reveal is a VRF.

## Invariants touched

- **10**: `vrf_verify` scheme 1 calls `ecvrf::verify` under suite octet
  `0x03`; the RFC vectors are re-run through the host in tests.
- **11**: nothing requires an oracle; `block_randomness` stays absent
  without one.
- **21** (retired): fuel charged before native work; calibrated, not
  guessed.
- **24**: host results are pure functions of inputs and committed state.
- Standing Order 4: no float, no clock, no map order.

## Activation

`FeatureSet::VRF_VERIFY = 1 << 33` (claimed in `adr-claims.txt`),
compiled `VRF_VERIFY_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**, together with
ADR-114 timers, which the pattern needs. Before the height `vrf_verify`
returns `-2` and the nested-call rule is not applied (a block applying it
early would produce a different state root; invariant 24 refuses it).

## Test plan

Red tests, `crates/vm/tests/vrf_host_tests.rs` and
`crates/node/tests/vrf_host_tests.rs`:

- `vrf_verify_accepts_the_rfc9381_vectors_through_the_host`
- `vrf_verify_refuses_a_tampered_proof_with_minus_one_and_no_trap`
- `vrf_verify_refuses_scheme_three_and_an_unknown_tag`
- `vrf_verify_charges_fuel_before_reading_guest_memory`
  (out-of-fuel traps with the pointers unread; the zkml test's shape)
- `a_nested_call_reading_block_randomness_gets_minus_four`
- `a_timer_callback_reading_block_randomness_sees_the_beacon_of_the_prior_block`
- `before_the_height_vrf_verify_returns_minus_two_for_every_scheme`
- `exploit_replays::a_wrapper_that_reverts_on_a_losing_draw_cannot_reach_the_lottery`

Calibration: `fuel_calibration_tests.rs` gains a VRF row; the measured
figure and command go to `reports/bench/vrf-host-<date>.md`.

## Open questions for Eric (economics only)

None. Fuel prices are measured, not chosen; no fee or reward changes.
