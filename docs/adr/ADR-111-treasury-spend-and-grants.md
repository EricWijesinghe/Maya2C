# ADR-111: Treasury spend path and vested grants through governance

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-28, CATALOG rows `treasury-ledger` and `grants-quadratic`, [prior-art/treasury-grants.md](../prior-art/treasury-grants.md), [ADR-029 fee market via `fees.rs`], [ADR-068](ADR-068-forever-mining.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), [ADR-113](ADR-113-vote-delegation.md); invariants 7, 12, 13, 17, 25, 28, 32

## Context

`crates/treasury` is a tested state machine that nothing calls:
"Not wired into the node: the treasury exists at genesis (`TreasuryGenesis`)
but its spending path is governance's" (`crates/treasury/src/lib.rs:1-7`).
`spending.rs:1-8` moves a spend `Proposed → Approved(k of stages) →
Executed` under a per-epoch limit and appends every transition to a ledger
(`Treasury::new(balance, epoch_blocks, epoch_limit, stages)`,
`spending.rs:73-88`; `propose` `:101`, `approve` `:128`, `reject` `:178`).
`vesting.rs:16` has `Grant` with `vested(height)` `:78`, `claimable` `:96`,
`claim` `:105`, `terminate` `:118`, `claw_back` `:131`. `features.toml`
lists `treasury-vesting-rules` as RESEARCH.

Genesis may fund a treasury account (`genesis.rs:93-101`, `TreasuryGenesis`
`:479`, at most `MAX_TREASURY_SHARE_BPS = 5_000` of supply `:511`), but
mainnet writes none (`deploy/genesis/mainnet/README.md:72`), and the fee
split's treasury share is zero (`genesis.rs:277`, `treasury_bps: 0`;
`crates/fee-market/src/split.rs:41-43`, `MAX_TREASURY_BPS = 5_000` in
`limits.rs:66`). Rewards come from `MINING_RESERVE`, never from a treasury
(ADR-068).

Governance can move numbers and schedule upgrades, nothing else:
`ProposalSubmission { voting_blocks, timelock_blocks, changes, upgrade }`
(`crates/node/src/core/governance_payload.rs:148-157`). Its floors are
compiled (`crates/governance/src/limits.rs:33-95`, invariant 12), and the
council may schedule a suite sunset and nothing else (invariant 34).

Cardano's treasury withdrawal is a governance action with a net-change
limit; Polkadot routes spends through size-tiered tracks; Optimism's
RetroPGF is an off-chain round that pays on chain (prior-art file).

**Finding that contradicts the brief.** The parity row reads "no spend path
(E3/E5)". True, and also: mainnet has **no treasury at all**, so the spend
path alone funds nothing. This ADR therefore defines the inflow as well as
the outflow, and the inflow is an economics decision left to Eric.

## Decision

**The treasury is an account with no key. It is funded by a governed share
of base fees and by plain transfers, and it is spent only by a governance
proposal that passes the existing vote, timelock and execution lifecycle,
bounded per epoch by a compiled ceiling. A spend is either immediate or a
vested grant the grantee claims.**

### The account

`TREASURY_ADDRESS = blake3::derive_key("maya2c treasury v1", chain_id)`,
derived like `FEE_COLLECTOR` (`fees.rs:37`): no key exists for it, so the
only way value leaves is the spend path below. A genesis
`TreasuryGenesis.address` that is not this derived address is refused once
the feature is compiled in, so a chain cannot have two treasuries. Mainnet's
treasury starts at zero.

### Inflow

- A new `ParameterKey::FeeTreasuryBps` (bounds `0..=MAX_TREASURY_BPS`,
  default 0) read by the fee split (`split.rs`) at every block; the split's
  proved identity `burned + treasury + tip == base_fee + tip` is unchanged.
  Invariant 17: read from state, never from the constant.
- Any transfer to `TREASURY_ADDRESS` is a donation.

### Outflow

`ProposalSubmission` gains `spend: Option<TreasurySpend>`:

```
TreasurySpend { recipient: Address, amount: U256, vesting: Option<Vesting { cliff_blocks: u64, duration_blocks: u64 }> }
```

A proposal with a spend goes through `Propose → CastVote → finalize →
timelock → execute` exactly as a parameter change does
(`crates/node/src/state/governance_exec.rs`), with two extra checks at
proposal and again at execution (invariant 12's twice rule):

1. `amount <= treasury_balance`.
2. `amount + spent_this_epoch <= treasury_balance_at_epoch_start * TREASURY_EPOCH_CAP_BPS / BPS_DENOMINATOR`,
   where `TREASURY_EPOCH_CAP_BPS = 1_000` (10%) is **compiled** in
   `crates/governance/src/limits.rs`, appears in no `ParameterKey`, and the
   epoch is the staking epoch (`staking_exec.rs:312`). A governed cap would
   be a cap governance can raise to 100% in one vote.

At execution: immediate spend credits `recipient`; a vested spend writes
`g:grant:` ‖ `proposal_id` → `Grant { grantee, total, cliff, duration,
claimed, start_height }` and moves nothing yet. The treasury balance is
debited at execution in both cases (the vested amount is escrowed in the
grant record, so a later proposal cannot double-spend it).

| TxKind | Rule |
|---|---|
| `ClaimGrant { id }` | Grantee only; credits `claimable(height)` per `vesting.rs:96`; `claimed += amount`. Nothing claimable is a **no-op** (invariant 7). |
| `TerminateGrant` | Not a transaction: a later proposal with `terminate: id` in its spend field; at execution, `claw_back` (`vesting.rs:131`) returns the unvested remainder to the treasury and the vested remainder stays claimable. |

### 256-bit arithmetic with a written bound

The epoch cap is `balance * cap_bps / 10_000`. Bound: `cap_bps <= 10_000`
(`BPS_DENOMINATOR`), and `balance` is below total supply, which ADR-068
keeps below `2^192` for lifetime emission plus the genesis allocation; the
check refuses a `balance >= 2^240` before multiplying, with the bound
written beside it, so the product is below `2^254`. Vesting uses
`vested = total * elapsed / duration`: `elapsed <= duration <= 2^64`, and
`total < 2^192` by the same bound, so the product is below `2^256`. One
multiplication, one division, no 512-bit intermediate (Eric, 2026-11-01,
option A).

### Grants rounds

A "round" is a set of spend proposals opened in the same voting window; the
chain does not know a round. Quadratic matching needs proof of personhood
(CATALOG `grants-quadratic`, `features.toml` personhood RESEARCH) and is
**out of scope**: with stake-weighted votes, matching is buyable.

## Alternatives considered

- **Multi-stage approvers (`spending.rs` stages: grants committee, council,
  timelock).** Each stage is a named party with a key, which is what the
  registry-without-admin-key reasoning rejects (`oracle/registry.rs:17-20`).
  The governance vote is stage one and the timelock is stage two; the
  council is not given a veto, because invariant 34 limits it to sunsets
  and adding a money power to a sunsetting body is the wrong direction.
  `crates/treasury::spending` stays the reference model for an off-chain
  grants committee that submits proposals.
- **A governed epoch cap.** See above; compiled.
- **Treasury funded from the mining tail.** ADR-068's conservation equation
  depends on rewards being transfers from `MINING_RESERVE`; redirecting
  part of them is an economics change beyond this ADR.
- **Spend as a separate `TxKind::ProposeSpend`.** A second proposal
  lifecycle to keep in step with the first. Rejected; one lifecycle.

## Consequences

- Public goods can be funded on chain, with every spend a proposal anyone
  can read and a ledger that is the chain itself.
- One new `ParameterKey`, one compiled cap, one prefix `g:grant:` under the
  governance layer (already committed), one new `TxKind::ClaimGrant` under
  `Module::Governance`, one optional field on `ProposalSubmission`.
- The fee split starts routing value to an account once `FeeTreasuryBps`
  is raised; until then the treasury is empty and the spend path refuses
  every amount.
- Explorer and site show the treasury balance, inflow bps and open grants
  (feedback: site follows every upgrade).

## Invariants touched

- **12**: `TREASURY_EPOCH_CAP_BPS` compiled; both checks at proposal and at
  execution.
- **13**: a spend is a number and an address, never a program.
- **17**: `FeeTreasuryBps` read from state.
- **25**: `g:grant:` under `g:`; undo covered.
- **28**: `ClaimGrant` under `Module::Governance`.
- **32**: the treasury is not a vault; `ClaimGrant` is not a transfer door.
- **7**: a claim with nothing vested is a no-op.

## Activation

`FeatureSet::TREASURY_SPEND = 1 << 19` (claimed in `adr-claims.txt`),
compiled `TREASURY_SPEND_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**. Before activation a
proposal with a `spend` field and a `ClaimGrant` are refused on admission
and on apply, and `FeeTreasuryBps` is not in the parameter table.

## Test plan

Red tests, `crates/node/tests/treasury_spend_tests.rs`:

- `a_spend_over_the_epoch_cap_is_refused_at_proposal_and_again_at_execution`
- `a_spend_passes_only_through_vote_timelock_and_execute`
- `the_treasury_address_has_no_key_and_a_transfer_from_it_is_unsignable`
- `a_vested_grant_debits_the_treasury_at_execution_and_pays_on_claim`
- `a_claim_before_the_cliff_is_a_no_op_and_moves_no_root`
- `a_terminated_grant_returns_the_unvested_remainder_to_the_treasury`
- `a_balance_at_two_to_the_240_is_refused_before_multiplying`
- `a_reorg_restores_the_treasury_balance_grant_and_epoch_spent`

`crates/treasury` property tests: `claimed <= vested(height) <= total`
across random claim and terminate sequences. Vectors (`spec/03-state.md`
TR-1..TR-3): a grant record; vested amounts at cliff, mid-way and end;
the fee split with `FeeTreasuryBps = 500`.

## Open questions for Eric (economics and pricing only)

1. `FeeTreasuryBps` at activation (0 as written; anything above 0 lowers the
   burn ADR-068 counts on).
2. `TREASURY_EPOCH_CAP_BPS` (10% of the balance per staking epoch as written).
3. Whether a failed proposal's `GovernanceProposalDeposit` goes to the
   treasury or burns (today it burns).
4. Default vesting for grants (cliff and duration), if any is mandated.
