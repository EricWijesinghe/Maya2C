# ADR-134: Appchains secured by Maya2C validators — options from "none" to a replicated-security framework

**Status:** OPTIONS FOR ERIC, 2026-11-12. Nothing is decided here. Every option but A is outside the ADR-016 launch scope and post-genesis.
**Date:** 2026-11-12
**Related:** parity row P-37 (and P-14), [prior-art/appchains.md](../prior-art/appchains.md), ADR-122 (cross-chain messaging over verified headers, branch `docs/adr-117-124-gaps`), ADR-125 (succinct chain-state proof) and ADR-126 (Bitcoin anchoring), branch `docs/adr-125-130-gaps`, ADR-132 (duties, this branch), [ADR-016](ADR-016-launch-scope.md), [ADR-027](ADR-027-dag-bft-in-the-node.md), [ADR-065](ADR-065-cross-chain-asset-routes.md), ADR-089 (finality light client, claimed); invariants 11, 12, 24, 25, 28, 29, 30

## Context

What Maya2C can do for another chain today:

- **Prove its own state to anyone.** A `StateLayer` proof against a
  finalized header (`crates/node/src/state/proof.rs:311`) is the
  exportable fact ADR-122 relies on for outbound messages; ADR-122 says
  plainly that "what verifies that proof on Ethereum does not exist".
- **Verify another chain's headers in state, for Ethereum.** ADR-122 puts
  an Ethereum beacon light client under `x:lc:eth:` in the reserved
  bridge layer `x:` (`state/commitments.rs:252`, ADR-065), delivers
  inbound messages with ordered nonces and a route cap, and is inbound
  only. The pattern (light client as state records, messages proven
  against them) is the reusable part; the BLS verification is
  Ethereum-specific.
- **Its light client is PoW-shaped.** `crates/light-client` verifies
  headers by proof of work and cumulative work fork choice
  (`crates/light-client/src/lib.rs:10-14`) plus `AccountProof`. The node's
  production consensus is DAG-BFT (ADR-027). An appchain that wants to
  verify Maya2C finality needs the finality light client of ADR-089
  (claimed, succinct phase 0) or ADR-125's STARK, not this crate.
- **L2 means payment channels.** `crates/l2-flash` is a state-channel
  network: open, exchange signed states off chain, settle
  (`crates/l2-flash/src/lib.rs:1-15`), REAL and verified
  (`features.toml:2782-2790`). It is not a rollup and not a framework for
  running another chain.
- **The suite registry decides what a key is.** Invariants 29 and 30: a
  signature suite is registered and sunset by schedule. An appchain
  validator key verified by Maya2C must be a registered suite, so an
  appchain inherits Maya2C's key agility for free and cannot use a key
  Maya2C has sunset.
- **No oracle by default** (invariant 11). "The appchain's state is X"
  must be proven, not reported.
- **ADR-016** does not list appchains in either table; memory
  `owner-directive-permanent-first-2026-11-07` keeps non-genesis work
  out of the launch.

**Findings that contradict the brief.**

1. The parity row cites invariant 24 (a block's id and roots cover its
   transactions). An appchain's blocks are not Maya2C blocks; 24 is
   reached only by the checkpoint *record* a Maya2C block carries, which
   24 already covers like any record. The invariants an appchain
   framework actually touches are 11, 12, 25, 28, 29 and 30.
2. P-14 in the matrix lists `l2-flash REAL/verified` under "rollup / L2
   framework". Channels are an L2 for payments; they do not make the row
   shipped. The row is "none" for rollups and "none" for appchains.
3. "Secured by Maya2C validators" has two readings the surveyed chains
   split on: Polkadot's relay validators *execute* parachain blocks
   (replicated security, paid with coretime); Avalanche L1s run their own
   validators and pay the primary network a continuous fee for
   registration and messaging (ACP-77), with no primary-network
   execution; Cosmos ICS v1 replicated the full set and is being removed,
   ICS v2 is opt-in. The brief's phrase matches Polkadot; the surveyed
   trend is toward Avalanche's shape.
4. ADR-122 is Ethereum-inbound. An appchain→Maya2C message path needs a
   light client for the *appchain's* consensus in Maya2C state. If the
   appchain uses Maya2C's own vote format and registered suites, that
   light client is "verify n-of-m ML-DSA signatures from a registered
   set", which is cheaper than the BLS path and has no post-quantum
   caveat. If the appchain uses anything else, each consensus needs its
   own verifier, as ADR-122 needed one for Ethereum.

## Options

### A. None: channels are the L2; appchains are sovereign and use ADR-122-style light clients if they want messages

An appchain runs its own validators with its own stake. If it wants to
read Maya2C it runs ADR-089's finality light client; if Maya2C wants to
read it, that is an ADR-122-shaped light client per consensus, each its
own ADR. No shared stake, no registration, no fee.

### B. Registered appchains with checkpoint anchoring (Avalanche L1 shape)

Prefix `ac:`, layer `StateLayer::Appchains`, breaker `Module::Appchains`:

- `ac:c:` ‖ chain_id(32) → `Appchain { owner, validator_set_hash, threshold: u16,
  genesis_hash, fee_balance: U256, last_checkpoint: u64, registered_at }`.
  The validator set is a list of `(suite, public_key)` under registered
  suites (29/30), committed by hash, with the list itself in a
  `RegisterAppchain` payload bounded by `MAX_APPCHAIN_VALIDATORS`.
- `ac:k:` ‖ chain_id ‖ height(8 BE) → `Checkpoint { state_root, block_hash,
  signers: bitmap }` landed by `SubmitCheckpoint { chain_id, height,
  state_root, block_hash, signatures }`; the node verifies `threshold`
  signatures from the registered set over `blake3("maya2c appchain
  checkpoint v1" ‖ chain_id ‖ height ‖ state_root ‖ block_hash)`,
  refuses a height ≤ `last_checkpoint`, and refuses a second checkpoint
  at the same height with a different root (the equivocation is
  recorded under `ac:e:` for the appchain's own slashing; Maya2C slashes
  nothing because the stake is not its own).
- **Continuous fee**: each checkpoint debits `appchain_fee_per_checkpoint`
  (governed) from `fee_balance`; a chain with an empty balance has its
  checkpoints refused until topped up (ACP-77's balance rule). The fee
  goes to the committee through the existing staking payout.
- **Messaging**: appchain → Maya2C is a `StateLayer` proof from the
  appchain against `ac:k:`'s `state_root`, delivered on ADR-122's
  ordered-nonce message path with `x:msg:route:` pointing at the
  appchain's `chain_id` — if and only if the appchain's state proof
  format is Maya2C's own (`state/proof.rs`), which holds when the
  appchain is built from `crates/node`. Maya2C → appchain is ADR-122's
  outbound record plus ADR-089's light client on the appchain side.
- Security: the appchain's own validators. Maya2C provides ordering of
  checkpoints, a canonical history anybody can prove against, key
  agility, and the message rail. It does not execute appchain blocks and
  does not put its stake behind them.

### C. Opt-in validator subset (Cosmos ICS v2 / partial set shape)

B plus: Maya2C validators may opt in (ADR-132 option B duty
`appchain:<chain_id>`) to be the appchain's validator set, signing
checkpoints with their Maya2C keys. A checkpoint equivocation recorded
under `ac:e:` by a Maya2C validator is a duty fault slashable under
ADR-132's caps. The appchain's continuous fee is the duty's payment.
Requires ADR-132 option B first.

### D. Replicated security: every Maya2C validator executes every appchain block (Polkadot relay shape)

Appchain blocks are carried in Maya2C blocks as a second payload, every
validator runs every appchain's state transition, and appchain state
roots are under Maya2C's root. Refused: apply time per block becomes the
sum of every appchain's, which no breaker (28) can bound, the Maya2C
block byte cap (33) becomes the appchains' shared cap, and the ADR-016
scope is gone. Polkadot bounds this with a fixed number of cores and a
coretime market; the Cosmos Hub is removing its version as technical
debt (prior-art file).

## Recommendation

**A at launch; B as the post-genesis shape; C only if ADR-132 option B
is adopted and an appchain asks for it; D refused.** Reasons:

1. B is what the surveyed trend converged on (Avalanche ACP-77: own
   validators, continuous fee to the primary network, no primary-network
   execution). It needs no stake export (ADR-132 stays at A), introduces
   no trusted party (signatures are verified, invariant 11), bounds
   apply time to `threshold` signature checks per checkpoint under a
   per-block cap, and reuses ADR-122's rail and the existing proof format.
2. B's hard requirement is honest: an appchain built from `crates/node`
   gets messaging; one built from anything else gets checkpoint anchoring
   and nothing more until a verifier for its proof format exists.
3. C is B plus ADR-132's slashing; decide it there.
4. There is no appchain. The CATALOG lists none; the matrix found none.
   A framework before a user is the kind of module ADR-016 defers.

## Consequences per option

| | A none | B registered + anchored | C opt-in subset | D replicated |
|---|---|---|---|---|
| Whose stake secures the appchain | its own | its own | opted-in Maya2C validators (capped) | all Maya2C stake |
| Maya2C executes appchain blocks | no | no | no | yes |
| New state | none | `ac:c:`, `ac:k:`, `ac:e:` | + `k:opt:` (ADR-132) | appchain roots under Maya2C's root |
| Apply-time cost | none | ≤ `MAX_CHECKPOINTS_PER_BLOCK` × `threshold` ML-DSA verifies | same | unbounded by Maya2C rules |
| Who pays | nobody | the appchain, per checkpoint, to the committee | same, as duty payment | coretime-style market, not designed |
| Messaging | per-consensus light client, own ADR | ADR-122 rail if the appchain is `crates/node`-based | same | native |
| Invariant 11 | holds | holds | holds | holds |
| ADR-016 scope | inside | outside; evidence row needed | outside | outside and refused |

## Invariants touched

- **9**: checkpoints are ordered by appchain height; no time.
- **11**: signatures verified by the node; no reporter.
- **12**: C only — committee seats lost to duty slashing are bounded by
  ADR-132's caps inside the governance range.
- **17**: `appchain_fee_per_checkpoint`, `MAX_APPCHAIN_VALIDATORS`,
  `MAX_CHECKPOINTS_PER_BLOCK` governed, read from state.
- **24**: untouched beyond "records are under the root".
- **25**: `ac:` → `RECORD_LAYERS`, `StateLayer::Appchains`, undo coverage;
  messages share `x:` with ADR-122.
- **28**: `Module::Appchains`; a tripped breaker refuses checkpoints and
  registrations, never fee-balance withdrawal by the owner.
- **29, 30**: appchain validator keys must be registered suites; a sunset
  suite's signatures stop counting toward `threshold` at the sunset
  height, which is the one way Maya2C's schedule reaches into an
  appchain and is stated as such.
- **33**: signature bytes in a checkpoint count toward the block byte cap;
  a hybrid signature is 11,165 bytes (`hal/radio-transport/src/lib.rs:5-8`),
  so a 7-of-10 checkpoint is about 78 KB — `MAX_CHECKPOINTS_PER_BLOCK`
  is a real limit, not a formality.

## 256-bit math

`fee_balance − appchain_fee_per_checkpoint` is a checked subtraction;
there is no multiplication. ADR-132's slash arithmetic applies to C.

## Test plan if adopted (B)

Red tests, `crates/node/tests/appchain_tests.rs`:

- `before_the_height_registration_is_refused_and_the_root_matches_master`
- `a_checkpoint_needs_threshold_signatures_from_the_registered_set`
- `a_signature_from_an_unregistered_or_sunset_suite_does_not_count` (29/30)
- `a_checkpoint_at_or_below_last_checkpoint_is_refused`
- `two_roots_at_one_height_record_an_equivocation_and_the_second_is_refused`
- `an_empty_fee_balance_refuses_checkpoints_and_topping_up_resumes_them`
- `the_owner_can_withdraw_fee_balance_with_a_tripped_breaker` (28)
- `checkpoints_per_block_are_capped_and_the_bytes_stay_under_max_block_bytes` (33)
- `a_message_from_an_appchain_is_delivered_only_against_a_landed_checkpoint_root` (with ADR-122)
- `a_reorg_restores_checkpoints_fee_balances_and_equivocation_records` (25)
- `exploit_replays::a_registered_set_that_colludes_can_rewrite_its_own_history_but_not_maya2c_state` — prints what the appchain can and cannot do.

Spec: an "appchains" section, vectors AC-1..AC-4; the checkpoint domain
string is a conformance vector.

## Activation

`FeatureSet::APPCHAINS = 1 << 40` (reserved in `adr-claims.txt`; unused
under A), compiled height `u64::MAX`, scheduled under ADR-082 after
`XCHAIN_MESSAGING` (bit 29) if messaging is wanted, and after
`STAKE_DUTIES` (bit 38) for C.

## Decision needed from Eric

1. A (sovereign appchains, no framework), B (registered appchains with
   anchored checkpoints and a continuous fee), or C (B plus opted-in
   Maya2C validators under ADR-132)? D is refused unless overruled.
2. Does "secured by Maya2C validators" in his brief mean execution (D) or
   anchoring plus key agility (B)? The ADR reads it as B and says so.
3. If B: `appchain_fee_per_checkpoint` at activation, and whether the fee
   goes to the committee (default) or the treasury.
4. Must an appchain be built from `crates/node` to get messaging, or
   should Maya2C fund verifiers for other proof formats (each its own ADR)?
5. Is a finality light client for Maya2C (ADR-089) to be delivered before
   any appchain work, since every option but A needs it on the appchain side?
