# ADR-099: Mempool admission applies the executor's payload rules

Status: Accepted (post-genesis, v2). Claim: `099` in `D:/Temp/claude-locks/adr-claims.txt`.
Related: ADR-027 (DAG-BFT builder), ADR-072 (block cap, first-fit), invariant 24.

## Context

`StateDB::select_applicable_within` decides which pooled transactions a block
takes. It discarded every refusal (`Err(_) => false`). `Mempool::validate`
checked balance, nonce, fee, signature and vault rules, but not the typed
payload rules that run in `stage_transaction` (a DID schema, a stamp with no
DID, a second registration). Such a transaction was admitted, stayed pooled,
passed `revalidate` every block, and was dropped at build with no reason.
Reported on the notary CLI, 2026-11-10 (draft PR #439).

## Decision

Admission applies the executor's payload rules. Block building is unchanged.

- `StateDB::admit_payload` stages the transaction through the executor's own
  `stage_transaction` on a throwaway overlay. The sender's nonce is set to the
  transaction's, so only the payload and the other executor rules decide.
- `Mempool::validate` calls it for every transaction except mining shares
  (whose work depends on the block's difficulty target, which admission does
  not know). `revalidate` uses the same validation, so a pooled transaction
  whose payload stops executing is evicted, and the eviction is logged with its
  reason.

## Alternatives considered

- Builder reports refusals and the mempool evicts them. Rejected for now: the
  builder runs on DAG sub-dags whose transactions do not come from this node's
  pool, so eviction would need a new hook through the consensus path
  (`consensus::bft::build_block`). It would also evict a transaction whose
  predecessor is still in flight.
- Admission on a fresh context with no nonce alignment. Rejected: a nonce ahead
  of the account is legitimate while its predecessor is pooled, and it would be
  refused.

## Consequences

- A payload that cannot execute on committed state is refused at `insert`, and
  the RPC returns the executor's reason.
- Trade-off: a payload that depends on a same-sender predecessor not yet
  committed (a stamp pooled behind its own DID registration) is refused until
  the predecessor commits. Before this change it was admitted and built only if
  the predecessor landed earlier in the same block.
- No consensus rule changes. Block validity and the state root are unchanged.
- `shortcut` (in `admit_payload`): admission uses the compiled shielded
  activation, not the chain's genesis value. See the code comment.
