# ADR-130: Agent registry and agent-to-agent payments — an agent is a session key with a card, payments are plain transfers under ADR-107's limits, receipts are signed off-chain

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled; depends on ADR-107 (session keys on the v9 frame) and PR #447.
**Date:** 2026-11-12
**Related:** parity row P-48, CATALOG rows `agent-wallets`, `personal-agent`, `mcp-server`, [prior-art/agent-registry.md](../prior-art/agent-registry.md), ADR-107 (smart-account promotion, branch `docs/adr-102-109-parity`), [ADR-086](ADR-086-wallet-v2-seedless-recovery-and-clear-signing.md) (payment requests), [ADR-030](ADR-030-vault-accounts.md), ADR-084 (MCP read-and-draft, PR #440), ADR-119 (data carrier, branch `docs/adr-117-124-gaps`), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 25, 28, 31, 32

## Context

Three pieces exist, none of them an agent:

- `crates/smart-account` (RESEARCH, working): `SessionScope { recipients,
  max_amount, expires_at }` with `allows(to, amount, height)`
  (`policy.rs:41-61`), `Action::AddSessionKey { suite, public_key, scope }`
  (`op.rs:41-47`), a daily limit on the account (`policy.rs:12-14`), and
  a bounded validation cost (`account.rs:17-22`). ADR-107 promotes
  session keys and limits into the node on the v9 account-authority
  frame through `verify_with_state`, evaluated before admission; a
  session key may sign a `Transfer` and nothing else; `RevokeSessionKey`
  ends it.
- `bins/maya2c-mcp` (`main.rs:1-15`): four tools — decode, review
  intent, explain error, `rpc_call` — and **state-changing methods are
  refused unless the node is on loopback**: an assistant drafts, a person
  signs (ADR-084). No agent identity, no key.
- Vault accounts (invariant 32) bound what an address moves per window
  by a different mechanism.

What ERC-8004, AP2 and x402 add on other chains (prior-art file): a
registry where an agent publishes a record of itself and its owner, a
signed mandate from a human that bounds what the agent may spend, and a
settlement step that produces a receipt a counterparty can check. The
survey also says adoption is marginal and budget enforcement "against
creative workarounds" is open.

**Findings that contradict the brief.**

1. The parity row cites invariant 31 (v7/v8 verify through `verify_at`).
   Agents sign v9 frames, which ADR-107 routes through
   `verify_with_state`; 31's *rule* (never through `Transaction::verify`,
   fail closed without a height) is what matters, and the text of 31
   gains v9 under PR #447, not here.
2. "Session keys, signed receipts" are two layers. Session keys are
   ADR-107's and this ADR adds none; receipts need no chain rule at all.
   What the row lacks a home for is the **registry**: who the agent is,
   who owns it, what it claims to do, and how to turn it off. That is
   the state this ADR adds, and it is small.
3. Spending limits enforced "by contract or SDK design" (the survey's
   phrase) are what ADR-107 replaces with chain-evaluated rules: a limit
   the agent's own code cannot route around because the node refuses
   the frame. This ADR relies on that and adds no second enforcement.

## Decision

**An agent is a session key on a smart account, plus a registry record
that names it. Payments between agents are ordinary transfers signed by
session keys under ADR-107's scope and daily limit. A receipt is a
document both agents sign off-chain, referencing the txid. The chain
stores the registry and enforces the limits; it reads no agent card, no
mandate and no receipt.**

### Registry (invariant 25), prefix `ag:`, layer `StateLayer::Agents`, breaker `Module::Agents`

- `ag:a:` ‖ `agent_id(32)` → `Agent { owner: Address, key_hash(32),
  card_hash(32), endpoint_hash(32), registered_at: u64, revoked_at:
  u64 }`. `agent_id = blake3("maya2c agent v1" ‖ owner ‖ key_hash)`, so
  an id is bound to one owner and one session key and cannot be
  re-pointed. `key_hash` must be a session key registered on `owner`'s
  `ka:s:` record (ADR-107) at registration; the registry refuses a key
  the account does not hold.
- `ag:o:` ‖ `owner` ‖ `agent_id` → empty: the owner's index, so a wallet
  lists its agents in one scan.
- `card_hash` is the BLAKE3 of an off-chain **agent card** (name,
  capabilities, protocols spoken, pricing text, contact), the ERC-8004
  shape; `endpoint_hash` the BLAKE3 of where to reach it. The chain
  stores hashes; a resolver (indexer, the MCP server) serves the
  documents and checks them against the hashes. Nothing on chain is
  prose (ADR-104's "metadata hash only" rule).

Three actions on the v9 frame, owner-signed (a session key cannot sign
them — ADR-107 restricts session keys to `Transfer`):

| Action | Rule |
|---|---|
| `RegisterAgent { key_hash, card_hash, endpoint_hash }` | `key_hash` ∈ `ka:s:owner`; id derived; refused if it exists. Fee: the frame's. |
| `UpdateAgent { agent_id, card_hash, endpoint_hash }` | owner only; key never changes (a new key is a new agent). |
| `RevokeAgent { agent_id }` | sets `revoked_at = height`; **also** deletes the `ka:s:` session record, so revocation is one transaction (the "kill switch" the CATALOG row asks for). A `RevokeSessionKey` without `RevokeAgent` leaves the registry record standing and the agent unable to pay, which is also acceptable. |

### Payments

A payment from agent A (owner X) to agent B (owner Y) is a `Transfer`
from X to Y's chosen receiving address, signed by A's session key, and
admitted iff ADR-107's rules hold: Y's address ∈ A's `recipients`,
`amount <= max_amount`, height ≤ `expires_at`, and X's daily limit has
room (`ka:w:` window sum). No new kind, no new wire version. What the
registry adds to the payment:

- **Discovery**: B's card and endpoint by `agent_id`, through a resolver.
- **A2A binding**: the off-chain invoice and receipt (below) carry both
  `agent_id`s, so a counterparty knows which owner stands behind a key
  and can check `revoked_at == 0` before shipping.

### Invoice and receipt (off-chain)

ADR-086's payment request v2 (`maya:<bech32m>?amount=&label=&message=&exp=&id=`)
is the invoice, extended with `agent=<agent_id>`; the paying agent puts
`id` into the transfer's reference where the wallet's request flow
already does. A receipt is:

```
Receipt v1 { invoice_id, txid, payer_agent, payee_agent, amount: U256,
             height, signature_payee over blake3("maya2c a2a receipt v1" ‖ fields) }
```

signed by the payee agent's session key (any key may sign bytes; the
chain restricts what *frames* it may sign). Either party may publish it
through ADR-119's data carrier if a public record is wanted; the chain
does not require it. The MCP server gains `resolve_agent` and
`verify_receipt` as read tools and keeps refusing writes (ADR-084).

### Human approval for large payments

"Asks before large acts" (CATALOG `personal-agent`) is ADR-107's
`delay_above`: a transfer above the threshold is refused as an
immediate transfer and goes through the delayed, owner-cancellable path
PR #447 has. The agent cannot lower the threshold; a session key signs
`Transfer` and nothing else.

### What this is not

- Not an agent runtime, a model, or any AI on a node (memory
  `identity-ai-native`; ADR-079 §1). The registry records a key and two
  hashes.
- Not reputation. ERC-8004's reputation and validation registries are
  off-chain concerns here; a counterparty reads receipts from an
  indexer. A reputation score on chain is a judgment on the payout
  path and is not added.
- Not a streaming/channel payment. x402-style per-request micropayments
  are a sequence of transfers under the daily limit; a payment channel
  is a separate ADR if ever wanted.

## Alternatives considered

- **Agent as its own account with a vault limit (invariant 32).** Every
  agent then needs funding, guardians and a delay record; the owner's
  session scope is the point (ADR-107 rejected the same shape).
- **Registry inside the agent card only (no chain record).** Then a key
  hash has no discoverable owner and no revocation anyone can check —
  the prior-art survey's exact criticism of a bare session key.
- **Receipts on chain as a kind.** 64+ bytes per payment for a document
  two parties already hold; ADR-119 carries it when wanted.
- **Mandates (AP2) as chain objects.** ADR-107's scope *is* the mandate,
  evaluated by the node; a second signed object would be a copy the
  node does not read.
- **A new wire version.** Nothing here changes a frame; v9 carries the
  actions as account-authority operations.

## Consequences

- Two records, three owner actions, two MCP read tools, one invoice
  field, one receipt format. No new kind for payments.
- Limits are enforced where ADR-107 enforces them; this ADR adds no
  enforcement path to audit.
- Revocation is one owner transaction and is visible to every
  counterparty through the registry.
- An agent's spend is bounded by `max_amount` × transfers under the
  daily limit, which the owner sets; the chain does not know what a
  "reasonable" agent budget is and does not try to.

## Invariants touched

- **25**: `ag:` layer, undo covered.
- **28**: `Module::Agents`; a tripped breaker admits no registry action
  (payments are `Transfer`s and are unaffected, by design: a registry
  outage must not freeze money).
- **31**: v9 frames stay out of `Transaction::verify`; the registry
  actions are owner-signed v9 operations through `verify_with_state`.
- **32**: untouched; a vault that is also an agent owner satisfies both
  limits (ADR-107's "stricter wins").

## Activation

`FeatureSet::AGENT_REGISTRY = 1 << 36` (claimed in `adr-claims.txt`),
compiled `AGENT_REGISTRY_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**, after ADR-107 (bit
15) in the same bundle. Before the height the three actions are refused
and no `ag:` record exists; receipts and MCP tools (off-chain) need no
height.

## Test plan

Red tests, `crates/node/tests/agent_registry_tests.rs`,
`bins/maya2c-mcp/tests/agent_tools_tests.rs`:

- `registering_an_agent_requires_a_session_key_the_owner_holds`
- `an_agent_id_is_bound_to_owner_and_key_and_cannot_be_re_registered`
- `a_session_key_cannot_sign_register_update_or_revoke`
- `revoke_agent_deletes_the_session_record_in_the_same_transaction`
- `a_payment_outside_the_session_scope_is_refused_before_execution` (ADR-107's rule, re-pinned from the agent side)
- `a_payment_above_delay_above_takes_the_delayed_path`
- `a_tripped_agents_breaker_refuses_registry_actions_and_not_transfers`
- `before_the_height_the_actions_are_refused_and_the_root_matches_master`
- `a_reorg_restores_the_registry_and_the_owner_index`
- `verify_receipt_accepts_a_payee_signed_receipt_whose_txid_is_in_a_block`
- `verify_receipt_refuses_a_receipt_from_a_revoked_agent_after_revoked_at`
- `exploit_replays::a_compromised_agent_key_moves_at_most_the_scope_before_revocation`
  — the real figure (`max_amount` × transfers inside the window) is
  printed, not asserted against a target.

Spec: the account-authority section gains the three actions and vectors
AG-1..AG-3.

## Open questions for Eric (economics only)

1. Registration fee: the frame's fee alone (default), or a deposit
   returned on revocation to discourage junk registrations.
2. Nothing else; limits are per-owner choices and no protocol parameter
   is introduced.
