# ADR-046: Universe-scale supply headroom — u256 balances, 18-decimal precision, cap decoupled from emission

- Status: Accepted (Eric, 2026-10-12)
- Supersedes: the Cap, decimals, and `INITIAL_EPOCH_REWARD` rows of [ADR-044](ADR-044-mining-rewards-lane.md)
- Scope: consensus-critical, pre-genesis only. Requires `rust-reviewer` + `security-reviewer`,
  a spec update, conformance vectors, and its own testnet soak before mainnet genesis.

## Context

The owner wants the largest representable supply ceiling that **no fork can meaningfully
exceed**, without diluting token value. Two facts shape the decision:

1. **The number is the weakest moat.** Supply size confers no value or capability — value is
   `demand ÷ circulating supply`, and a bigger cap just redenominates. A copycat can always set
   a bigger constant, so "be the biggest number" is unwinnable *unless* you reach a ceiling that
   is physically meaningless to exceed.
2. **`u256` is that ceiling.** It represents up to ~1.16×10^77 base units — near the count of
   atoms in the observable universe (~10^80). `u512` (~10^154) represents vastly more units than
   there are atoms, i.e. nothing real. So `u256` is the maximum width that *means* anything; past
   it there is nothing for a fork to out-number.

`u64` cannot carry this: at 18 decimals `u64::MAX` is ~18 whole coins, unusable. So the width
must widen, and the right width — for the owner's stated anti-fork goal — is `u256`, not the
engineering-thrifty `u128` (which a fork could itself exceed with `u256`).

Financial safety is preserved by **decoupling the cap from emission**: the cap is an unreachable
ceiling; real circulating supply is governed by a controlled emission schedule, so inflation and
miner-reward value stay healthy regardless of how large the ceiling is.

## Decision

> **Crypto-path note (verified 2026-10-12):** the *active* state commitment uses the BLAKE3
> backend (`state/stateless.rs` → `sparse::open(&Blake3, …)`), which accepts a 256-bit value with
> no change. The lattice/Ring-SIS backend, whose leaf-bit budget a u256 value overflows, is a dark
> path (`STATELESS_ACTIVATION_HEIGHT = u64::MAX`). Its budget is widened (`LEAF_COLUMNS 2→3`) only
> so the tree compiles; that lattice-parameter change needs crypto review **before stateless
> activation**, not before genesis — it is not on the mainnet consensus path.

1. **Balance integer width `u64` → `u256`** across the ledger (~38 files: `state/account.rs`,
   `state/asset.rs`, `fee-market`, DEX, governance, shielded pool, settlement, mining_exec, the
   conservation invariant guard, RPC amount types). Serialization widens `[u8;8]` → `[u8;32]`.
   Implementation type: `primitive-types::U256` (fixed-size, `no_std`, battle-tested across the
   Ethereum/Substrate ecosystem; constant-time-friendly, unlike arbitrary-precision BigInt, which
   stays banned in consensus). New core dependency → must pass `cargo deny` and land in
   `[workspace.dependencies]` ([ADR-005](ADR-005-dependency-unification.md)).

2. **Decimals = 18** (convention, not a hard constant — `supply.rs` already fixes none). Ties the
   highest mainstream (Ethereum). **Not higher:** >18 breaks wallet/exchange/explorer interop for
   zero real benefit (nothing transacts below 10^-18). u256 + 18 dp gives ~1.16×10^59 whole coins
   of headroom.

3. **`MAX_SUPPLY` = `U256::MAX`** — an unreachable, unbeatable ceiling. `check_supply` still
   enforces `total ≤ MAX_SUPPLY` (vacuous today, kept for the day something mints to a limit).

4. **Emission decoupled from the cap.** Drop ADR-044's "`INITIAL_EPOCH_REWARD` set so half the cap
   issues before the first halving" rule — meaningless against a u256 cap. Replace with a fixed,
   value-calibrated schedule: a chosen `INITIAL_EPOCH_REWARD`, halving every `HALVING_EPOCHS`,
   plus a **small perpetual tail (~1%/yr)** so miner/validator reward never reaches zero. Exact
   `INITIAL_EPOCH_REWARD` and tail rate are genesis parameters, set and soak-tested before mainnet;
   proposed defaults recorded in DECISIONS.md, adjustable pre-genesis.

## Consequences

- **Unbeatable, permanent ceiling:** nothing meaningful is higher than u256, so the anti-fork
  concern is closed for good.
- **No financial damage:** the cap does not drive value; the controlled emission does. A fork that
  "maxes the number" gains nothing (nothing real is higher); a fork that maxes *emission* instead
  just hyperinflates and destroys its own token.
- **Cost:** each balance is 32 B instead of 8 B (more storage and slightly more CPU per balance);
  a large, security-reviewed refactor; a new core dependency.
- **Timing:** must land, be reviewed, and soak **before mainnet genesis** — it cannot change after.
  It is a dedicated effort, not merged blind into an open mainnet-gate branch.

## Amendment 2026-10-13: every amount is 256-bit, not only balances

The first implementation widened **balances** and left every amount a
transaction carries at `u64`: transfer outputs, the fee, stake bonds and
delegations, HTLC locks, DEX orders, governance spends, and genesis
allocations. At 18 decimals `u64::MAX` is about 18.4 coins, so no single
transaction could move more than that. The options put to Eric were:

1. widen every amount to `U256` (a wire-format change), or
2. keep `u64` amounts and use 9 decimals.

**Eric chose option 1: "Widen every amount to 256-bit."** (2026-10-13, relayed
from the parallel session maya2c-c9.)

- **Wire format:** each amount becomes 32 bytes little-endian (+24 bytes per
  amount). That changes the transaction encoding pinned by invariant 31, so it
  needs a spec update (`spec/02-transactions.md`), regenerated encoding and
  state-transition vectors, and the reference implementation (`crates/spec-ref`)
  widened with its own 256-bit type, not the node's.
- **Arithmetic:** output totals are summed in `U256` with checked arithmetic.
  STF-2's "sum past `u64::MAX`" becomes "sum past `2^256 − 1`".
- **JSON:** every amount on the wire is a base-10 string (`amount_serde`),
  never a JSON number.
- **`fee-market`:** stays dependency-free for Kani (invariants 1, 6, 12) and
  gets its own four-limb 256-bit type for `MAX_SUPPLY = 2^256 − 1`.
- **Before genesis only**, like the rest of this ADR.
