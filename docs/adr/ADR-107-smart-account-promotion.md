# ADR-107: Promote smart accounts (session keys, spending limits) from RESEARCH

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled. Depends on PR #447 (`postgen/account-authority`, ADR-086 P3).
**Date:** 2026-11-12
**Related:** parity row P-5, [prior-art/account-abstraction.md](../prior-art/account-abstraction.md), [ADR-086](ADR-086-wallet-v2-seedless-recovery-and-clear-signing.md), [ADR-056](ADR-056-guardian-recovery-transactions.md), [ADR-030](ADR-030-vault-accounts.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 25, 28, 29, 31, 32

## Context

`crates/smart-account` is a complete state machine with real ML-DSA-65
signatures in its tests and is **not a node transaction type**
(`crates/smart-account/src/lib.rs:26-30`; `features.toml` id `smart-account`,
core RESEARCH working). It holds: keys registered once with weights and a
threshold (`account.rs:34-65`), bounded validation cost `MAX_VALIDATION_COST`
40 with a per-suite `verify_cost` (`account.rs:17-22`), `Policy` and
`SessionScope { recipients, max_amount, expires_at }` with
`allows(to, amount, height)` (`policy.rs:12-61`), `Action::AddSessionKey`
(`op.rs:41-47`), guardians (`recovery.rs`), and `FeeSpec` (`fees.rs`).

PR #447 (ADR-086 phase P3) lifts the **authority** part into the node, dark:
wire version 9 account-authority frames, prefix `ka:` under
`StateLayer::AccountAuthority` (tag 18), `Module::Account` (tag 13), feature
bit 9, with policy digest and guardian recovery records. It does **not** carry
session keys or spending limits; those are what this ADR promotes. Vault
accounts already enforce a per-window limit by a different mechanism
(invariant 32, `crates/node/src/state/vault.rs`).

## Decision

**Session keys and spending limits become chain-evaluated rules on the v9
account-authority frame. Verification of a v9 frame gains a state read; it
stays out of `Transaction::verify` and out of `verify_at`, which are
stateless, and goes through one new height-and-state entry point that the
mempool and block execution both call.**

### What changes in `Transaction::verify`

Nothing in its body. `verify` (`transaction.rs:360-375`) already refuses
every frame that is not hybrid v5/v6; PR #447 extends that refusal to v9.
This ADR keeps the rule: **a v9 frame is never accepted by `verify`**. The
text of invariant 31 gains "account-authority (v9)" if PR #447 has not
already added it.

### What changes in `verify_at`

`verify_at` (`suite_tx.rs:203`) is stateless: it takes a height, the suite
policy and the chain tag. A v9 frame names keys by hash and resolves them
from `ka:` records (PR #447), so it cannot be verified without state. PR
#447 therefore verifies v9 in the apply path. This ADR names that path:

```
Transaction::verify_with_state(&self, height, policy, chain, state: &dyn AuthorityReader) -> Result<()>
```

- For v5..v8, v10, v11 it calls `verify_at` and ignores `state`.
- For v9 it: reads the account's current key set and policy from `state`
  (`ka:a:`); refuses if the frame's key hashes are not registered or their
  weights do not reach the threshold; charges `verify_cost(suite)` per
  signature against `MAX_VALIDATION_COST` **before** verifying any signature,
  so a frame cannot buy unbounded verification work; verifies each signature
  under `check_admissible(policy, suite, height)` (invariant 29/31); then
  applies the **policy rules** below.
- The mempool calls it with the tip's state; block execution with the
  parent's state. Both are the same committed state for the same parent, so
  they agree, which is the property invariant 31 protects.

### Policy rules evaluated at verification

Evaluated on the frame's outputs and kind, before execution, with the
account's policy from state:

| Rule | Source | Check |
|---|---|---|
| Session key | `SessionScope` (`policy.rs:41-61`) | A signature by a session key counts only if every output recipient is in `recipients`, every output `amount <= max_amount`, `height <= expires_at`, and `kind` is `Transfer` (a session key never signs a key change, a recovery or an upgrade). |
| Spending limit | `Policy` daily limit (`policy.rs:12`, `BLOCKS_PER_DAY` 43,200 at `:7`) | Sum of outputs in the window ending at `height` plus this frame's outputs `<= limit`. The window sum is a `ka:w:` record (below). |
| Large-transfer delay | `Policy` | An output over the delay threshold is refused as an immediate transfer; it must go through the delayed path PR #447 already has (`Pending`, `account.rs:54`). |
| Allow-list | `Policy` | If set, every recipient must be listed. |

A frame that fails a policy rule is **refused**, never a no-op, and nothing
is written. Policy evaluation reads only committed state and the frame: no
clock, no node-local value (invariant 28's rule for the guard applies here
for the same reason, a split).

### State (invariant 25)

Under PR #447's `ka:` prefix and layer, two record kinds are added:

- `ka:s:` ‖ `account` ‖ `key_hash` → `SessionScope`, written by
  `AddSessionKey` and deleted by `RevokeSessionKey` or by expiry on first
  use after `expires_at`.
- `ka:w:` ‖ `account` ‖ `window_start(8 BE)` → spent in window (`U256`).
  One record per window, `checked_add` only.

`F7` from the `features.toml` note (policy day length) is settled by this ADR
as `BLOCKS_PER_DAY = 43,200`, a block count, not a clock.

### Relation to vault accounts

A vault (ADR-030) and a smart account are two mechanisms for one idea. This
ADR does not merge them: a vault is a per-address record with a delay and
guardians; a smart account is a key set with policies. An account may be
both only if the vault's limit and the policy limit are each satisfied; the
stricter wins by construction. Merging is a later ADR, after both have run.

### Fees

`FeeSpec` (`fees.rs:22`) is **not** promoted. A v9 frame pays like any other
(output to `FEE_COLLECTOR`) or is sponsored under ADR-106. `FeeSpec::Token`
stays RESEARCH.

## Alternatives considered

- **Verify v9 inside `verify_at` with a state argument.** Makes a stateless
  function stateful for every caller, including the light client's
  `verify_block_under` (ADR-082 M2). Rejected; the new entry point is explicit.
- **Evaluate policy at execution rather than verification.** A frame that
  passes verification and fails execution consumes mempool space and a
  nonce decision; evaluating before admission keeps refusals cheap.
- **Session keys as separate accounts with a vault limit.** Reuses invariant
  32 but forces every app to create a vault per user; the scope is the
  point. Rejected.
- **Promote `FeeSpec::Token` too.** Needs an oracle price in consensus
  (invariant 11, a trusted party). Out.

## Consequences

- `features.toml` `smart-account` moves from RESEARCH to REAL `working` once
  the tests below pass, with `paths` adding `crates/node/src/state/account_authority*`.
- One new verification entry point; every caller of `verify_at` in the
  mempool and apply path switches to `verify_with_state` in one change
  (invariant 31's "every caller in one change" rule).
- Two record kinds under `ka:`; the undo journal covers them.
- `MAX_VALIDATION_COST` becomes consensus; changing it is an upgrade.
- Wallets gain scoped session keys (point-of-sale, agents) and a
  permission-center view (CATALOG `permission-center`).

## Invariants touched

- **31**: v9 verifies only through `verify_with_state`; `verify` and
  `verify_at` refuse it.
- **29**: every key is a registry suite under `check_admissible`.
- **25**: two new record kinds under the existing `ka:` prefix.
- **28**: `Module::Account` (PR #447) covers session and window records.
- **32**: a vault-and-account address satisfies both limits.

## Activation

Reuses `FeatureSet::ACCOUNT_AUTHORITY = 1 << 9` (PR #447) for the frame, and
adds `FeatureSet::ACCOUNT_POLICY = 1 << 15` for session keys and limits, so
PR #447 can activate without this ADR. Compiled `u64::MAX`; scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**, no earlier than bit 9.

## Test plan

Red tests, `crates/node/tests/account_policy_tests.rs`:

- `a_v9_frame_is_refused_by_verify_and_by_verify_at`
- `a_session_key_pays_only_listed_recipients_under_its_cap_before_expiry`
- `a_session_key_cannot_sign_a_key_change_or_a_recovery`
- `the_daily_window_sum_refuses_the_output_that_crosses_the_limit`
- `validation_cost_is_charged_before_any_signature_is_verified`
- `mempool_and_block_execution_agree_on_a_policy_refusal`
- `a_reorg_restores_the_window_sum_and_session_set`
- `a_vault_that_is_also_a_smart_account_obeys_the_stricter_limit`

Mutation check as PR #447 did: accept a session key for an unlisted
recipient and record which tests fail. Vectors (`spec/02-transactions.md`
AA-7..AA-10 after PR #447's AA-1..AA-6): a session-scoped frame's bytes; a
window record; a refusal at `limit + 1`.

## Open questions for Eric (economics and pricing only)

1. Does a v9 frame pay a per-signature surcharge (validation work is up to
   40 cost units), or bytes only?
2. Default daily limit and large-transfer threshold the wallet proposes for a
   new account (not consensus; wallet default).
