# ADR-062: Choose the accounts tree and the staking record layout before genesis

Status: Accepted (2026-10-27, Eric: both, measure first). Staking (b) and accounts (b) are built behind measurement (`cargo xtask bench-tps` before and after, and block-apply time against account count); what ships in the re-genesis is what the numbers justify. Originally Proposed 2026-10-26.

## Context
The freeze-readiness optimisation review (2026-10-26) found two costs that grow
with chain size and are locked into the state-root format.

1. **Accounts tree.** `StateDB::root_with_overlay` (`state/db.rs`) re-reads every
   account each block, hashes every leaf and rebuilds the whole dense binary
   tree (`state/merkle.rs`). Cost per block is O(all accounts), paid by the
   producer and every validator. A sparse tree already exists
   (`state/stateless.rs`, `sparse::root`), switched on by
   `STATELESS_ACTIVATION_HEIGHT`, currently `u64::MAX` (`docs/stateless.md`).
2. **Staking record.** `state/staking.rs` stores every validator and delegation
   in one value under one key, decoded and re-encoded whole on every staking
   transaction and each epoch boundary. Someone who mass-delegates dust makes
   every staking transaction costlier. The value also outgrows the old 65,536
   byte decode cap (fixed in the same change as ADR-061).

## Options
- **Accounts:** (a) keep the dense tree, tune the I/O only; (b) activate the
  sparse tree from genesis, which allows incremental update and witness-based
  stateless verification, at the cost of resolving the open questions in
  `docs/stateless.md` (how witnesses travel; the end-of-block passes).
- **Staking:** (a) keep one record; (b) one record per validator and per
  delegation, key-ordered, so a transaction touches only its own records.

## Recommendation
Staking (b) and accounts (b), decided together and shipped in one re-genesis,
each behind its own measurement: `cargo xtask bench-tps` before and after, and a
block-apply time against account count (MISSING today). Do not ship a state-root
change that has not been measured.

## Consequences
Light-client proofs and `spec-ref` depend on the tree shape, so the spec and
conformance vectors change with it. The live testnet cannot follow either; both
need the planned re-genesis.

## Amendment 2026-11-13: per-block work against stored records

The 2026-10-28 deferral of both halves (DECISIONS.md) assumed state grows
organically and said to revisit "when accounts pass ~50k or block time is
threatened". A sweep of every end-of-block pass and the root, after PR #500
found the same pattern in governance, measured it against records an attacker
creates. Test profile (node at opt-level 0), median of 3,
`state::scan_bounds_tests`, report `reports/sessions/2026-11-13-per-block-scan-bounds.md`.

**Fixed in this change** (`fix/per-block-scan-bounds`, no key layout change):

| Site | Before | Bound now | Measured at the bound |
|---|---|---|---|
| `settle_sealed`: one reveal height could inherit 255 × 128 = 32,640 envelopes, and `merged_records` walked the whole overlay per envelope | 18.95 s at 32,640 | `MAX_SEALED_PER_REVEAL_HEIGHT` = 128, checked at submission; `Overlay::records_under` ranges the overlay | 0.39 ms at 128 (183 ms at 32,640 with the range alone) |
| `settle_trading` rebuilds every touched book whole; the per-book ceiling times the pairs one block touches was the real bound | 549 ms for 16 full books; a block at the fee target touches about 190 | `MAX_BOOK_ORDERS_PER_BLOCK` = 16,384 resting orders across touched books; the first book is always admitted | 86 ms for 4 full books |
| The staking record is one value, decoded and re-encoded by every block and every staking transaction; the cap was 8 MiB | 87,000 delegations (8.35 MB): 250–289 ms a block, 152 ms a transaction | record cap 1 MiB (about 10,900 delegations); `MAX_STAKING_TX_PER_BLOCK` = 16, evidence exempt | 10,000 delegations (0.96 MB): 28 ms a block, 15 ms a transaction |

Each refusal is a transaction failing, never a block. Rejected: a counter
record per reveal height (a new key for a bound the scan of at most 128 keys
already gives); skipping the book pass for pairs touched only by swaps (changes
matching semantics, and a placement touches a pair as cheaply); caching the
decoded staking record across a block (`select_applicable` clones the overlay
per non-transfer transaction, so the clone would cost what the decode did).

**Not fixed here, CRITICAL:** the state root itself. `root_with_overlay`
re-reads and hashes every account and every committed record each block:
1.3–1.8 µs an account and 2.1 µs a 200-byte record on the test profile
(131 ms at 100,000 accounts), 0.48 µs an account in release
(`reports/12-performance.md`). One output to a fresh address creates an
account; a 4 MiB transfer carries about 65,000 outputs, so 2.1 million
accounts (about 1 s of root in release) take some 26 full blocks, and the fee
from the floor costs well under a millionth of a coin (about 1,000 coins even
at the compiled ceiling). That is ADR-062a (an incremental root) or a
state-creation price, both decisions for Eric. Every per-record bound above
still leaves its records in this fold.

**Residual, for Eric:** the default `min_delegation` is 10 base units, so the
1 MiB record (and the old 8 MiB one) can be filled with dust for fees alone,
after which every new delegation is refused until it drains. A real minimum,
or ADR-062b, ends that. ADR-062b also removes the 10,900-delegation ceiling
this amendment introduces.
