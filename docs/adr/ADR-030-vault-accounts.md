# ADR-030: Vault accounts — delayed withdrawals, guardian cancel

**Status:** Accepted
**Date:** 2026-09-27
**Implements:** Master Prompt 22 §("opt-in vault accounts"), Master Prompt 28 §2–3 (PQ vaults)

## Context

Master Prompt 22 asks for opt-in vault accounts: "outgoing transfers above a
limit wait N hours and can be cancelled by guardians. Normal accounts stay
instant and final." Master Prompt 28 builds its quantum-safe harbor on them:
value held under ML-DSA / SLH-DSA keys *and* a policy that gives the owner
time to react when a key is stolen. `crates/smart-account` has the delay and
guardian logic as a library, but no transaction reaches it, so on the chain
there has been no vault at all.

## Decision

A new transaction kind, `TxKind::Vault` (tag 52), with four actions:

| Action | Sent by | Effect |
|---|---|---|
| `Configure { delay_blocks, limit, guardians }` | the account | Opts in. On an account with no vault it takes effect at once. On a vault it is *queued* for `delay_blocks` of the **current** configuration, and a guardian can cancel it. |
| `Request { to, amount }` | the vault | Moves `amount` from the balance into escrow (`q:w:`), payable to `to` at `height + delay_blocks`. |
| `Execute { owner, id }` | anyone | After the delay, pays an open request's escrow to its recipient. |
| `Cancel { owner, id }` | a guardian, or the owner | Before execution, returns the escrow to the vault, or drops a queued reconfiguration. |

**What a vault account may send:** plain transfers and vault actions, whose
outputs together move at most `limit` **per window of `delay_blocks`**.
Nothing else is allowed: no HTLC lock, swap, order, stake, contract call or
channel. Each of those moves value out by another door, and a thief holding
the key would use it. A vault is a custody account; trading happens from an
ordinary account the vault pays.

Two rules came out of review, before this landed:

- **Fee outputs count.** They were excluded at first, and fees have no upper
  bound, so a stolen key could send the whole balance to the fee collector in
  one instant transaction. Nothing reached the thief, but everything left the
  owner. Fee outputs now count toward the limit above 4x the fee the rule
  requires (wallets pay 2x).
- **The limit is per window, not per transaction.** Per transaction, a thief
  could queue thousands of in-limit transfers into one round. The record keeps
  `window_start` and `window_spent`, charged at execution. The window resets
  after `delay_blocks`.

**Why reconfiguration is delayed:** otherwise the first thing a thief does
is set `delay_blocks = 0`.

**Why the escrow leaves the balance at request time:** a request that only
*reserved* funds could be raced by an in-limit transfer. Escrow is counted by
the conservation guard (`q:w:` records), like HTLC escrow (`h:lk:`). A
settled request is **deleted**, so a vault's records stay bounded by its open
requests, and a request cannot be settled twice. The history is in the
blocks.

**Circuit breaker.** `Module::Vault` (tag 12): configure, request and execute
can be halted like any module, and held value simply stays escrowed. A
**cancel is never halted**. It returns escrow to its owner and is the
guardian's one defence during the delay, so halting it would hand a thief the
race. This is the HTLC split, for the same reason.

**State.** `q:` is a new record layer (`StateLayer::Vault`, tag 16) under the
state root (invariant 25). Layers fold only when non-empty, so every chain
that never configures a vault keeps exactly the root it had. The undo journal
records `q:` like every generic record, so a reorg reverts vault actions.

**Activation: genesis.** Tag 52 could not be decoded before this change, and
the transfer restriction applies only to accounts that configured a vault,
which none could. So no block's meaning changes, the same argument as
hash-lock HTLCs at height 0.

## Bounds

At most 16 guardians. `delay_blocks` from 1 to 1,000,000. At most 64 open
requests per vault, so escrow is not an unbounded scan.

## Honest limits (stated in the CLI and in reports/28)

- A vault protects against **theft of the owner's key**: the thief's
  withdrawals wait, and a guardian cancels them. It does not protect against a
  thief who also holds a guardian key, or against transfers under `limit`,
  which are instant by design.
- A bridged asset in a vault is only as safe as the weakest of the source
  chain, the bridge route and Maya2C (Master Prompt 28 §2). No outside-asset
  route exists yet, so today's vaults hold the native coin.
- Guardians are single approvers: any one of them can cancel. A threshold of
  guardians is a follow-up, on top of v8 multisig accounts.
- A thief with the owner's key can open up to 64 requests at once. Each is
  escrowed and safe, but a guardian must cancel each one within the delay.
- At `height == ready_height` a guardian's cancel and an execute are both
  valid, and whichever lands first wins. Guardians act before maturity, not
  at it.
- A second `Configure` while one is queued replaces the queued one, and the
  delay restarts from the current policy. Only the vault's key can do it, so
  it grants nothing new, and a guardian can still cancel the replacement.

## Consequences

- `stage_transaction` gains a vault check before any debit.
- `invariant_guard::conservation` folds open withdrawals.
- RPC `vault_get(address)` reports the configuration and open requests.
