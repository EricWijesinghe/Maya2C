# ADR-029: The fee market goes live — fees are signed outputs

**Status:** Accepted
**Date:** 2026-09-27
**Implements:** ADR-016 launch blocker 3; Master Prompt 3 ("fee market is active")

## Context

`crates/fee-market` has held an adaptive base fee, a split and a supply bound
since Master Prompt 3, activated nowhere (`FeeConfig::DISABLED`). Turning it on
needed three things the crate could not supply: a way for a transaction to
state what it pays, somewhere for the burn to go, and someone to receive tips.
The crate's own design used a `TxFee { max_fee, max_tip }` per transaction —
which would mean a new transaction wire version for every one of the three
formats (hybrid v5/v6, suite v7, multisig v8), their signing bytes, the TS
verifier and the conformance vectors.

## Decision

**A fee is an ordinary signed output to `FEE_COLLECTOR`**, a derived address
nobody holds a key for. Every transaction format already signs its outputs, so
the amount is the sender's explicit consent — the property `max_fee` exists
for — with no wire change. The fee is exact rather than a cap: what a wallet
signs is what leaves the account.

Per transaction: `offered = Σ outputs to FEE_COLLECTOR` must be at least
`base_fee × serialized size`, else `FeeTooLow` and the block is invalid (the
base fee is known from the parent, so a producer that includes an underpaying
transaction built an invalid block). A typed-payload transaction, which may
not carry transfer outputs, may carry outputs to the collector.

Per block: the base-fee part of everything collected moves from the collector
to `FEE_SINK` (burned: supply conserved, circulation falls), and the base fee
steps toward `target_block_bytes` with `maya_fee_market::next_base_fee`.

Per staking epoch: the collector's balance — the tips — is the reward pool
`maya-staking` pays out (ADR-028). There is no treasury share at launch
(`treasury_bps = 0`); governance can add one.

**Presence is activation.** `genesis.bft.fees` seeds `k:fee`; without it no
record exists, no rule runs, and no root moves. Parameters are checked against
the fee market's compiled-in limits at genesis.

Wallets learn the base fee and collector from RPC `get_fee_info`;
`l1-wallet send` adds the fee output automatically (twice the base fee on the
signed size; the excess is the tip). The mempool refuses an underpaying
transaction before relaying it.

## Consequences

- MP03's "fee market is active" and ADR-016 blocker 3 hold on any genesis that
  configures `bft.fees`.
- Validators are paid from tips. Emission remains nil — the brief's
  economics (Master Prompt 18) decide whether to add one.
- The fee is exact, not a cap, so a wallet overpays by its safety margin; the
  overpayment is a tip, not lost.
- Not adopted: the neural base-fee rule (`neural_activation_height` stays
  `u64::MAX`), because its model was trained on synthetic traffic.

## Evidence

`crates/node/tests/fee_market_live_tests.rs`: an unpaid and a one-short
transfer are refused; a paid transfer lands, burns exactly `base_fee × size`,
leaves the tip in the collector, and conserves value; empty blocks walk the
base fee down and never below its floor.

## Amendment 2026-11-14: the neural fee model leaves the launch binary

**Decision (Eric 2026-11-14, founder-brief audit Q19, option A).** The neural
base-fee gain is no longer "not adopted but present"; it is out of the node.
Until this amendment `maya-fee-market` carried the 6-16-1 integer model
(`src/model/`), the one-sided rule (`src/rule.rs`) and a second switch,
`FeeConfig::neural_activation_height`, at `u64::MAX` in every configuration,
and the node carried the block-feature extractor (`src/neural_gas/`). None of
it ran, but all of it was compiled into the binary a genesis freezes.

- The model, the rule, their Kani envelope proof and their tests moved to a
  new crate, `crates/neural-fee` (`maya-neural-fee`), class RESEARCH. Its
  only dependant is `bins/neural-gas-trainer`, which writes its weight file.
- `FeeConfig` lost `neural_activation_height`, `TESTING_NEURAL` and
  `is_neural_active`; `apply_block_fees_with_features` and
  `FeeError::MissingFeatures` are gone. `apply_block_fees` computes the
  linear step directly, as it always did on every network.
- The node's `neural_gas` module, `tests/neural_fee_tests.rs` and the
  `gas_predictor` bench are deleted. They are in history at `3735252d`.
- Check: `cargo tree -p custom-l1-node -i maya-neural-fee` finds no path, and
  `tests/fee_market_tests.rs::the_launch_binary_carries_no_neural_fee_model`
  fails if the node manifest names the crate or either half comes back.

**Why.** "Models never on consensus" is the owner's identity rule, and
dormant code at `u64::MAX` is still code a reviewer of the launch binary has
to read, a field the genesis config must carry, and an attack surface if a
schedule ever named the height. Removing it costs nothing at launch: the
linear rule is the only one any network has run.

**Behaviour.** Unchanged. No network ever set the height, `FeeConfig` has no
serialized form, and no state record or wire type named the field, so no
state root, vector or encoding moves. The fee-market tests and the node's
fee tests pass as before.

**Reviving it** is a hard fork: a new ADR, the dependency added back to the
node, a feature extractor, and an activation height in the upgrade schedule
(invariant 34) — plus a model trained on real traffic, which ADR-016 already
required.
