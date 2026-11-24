# ADR-129: Verifiable-compute job market and inference receipts — ADR-079's rung (a) as records, a receipt format, and the `kind` byte that does not exist yet

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`) for the records and the claim kind, dark until scheduled; receipts and the off-chain worker are W3 tooling. Depends on ADR-079 R0/R2 and its invariant-11 decision.
**Date:** 2026-11-12
**Related:** parity row P-47, [prior-art/verifiable-inference.md](../prior-art/verifiable-inference.md), [ADR-079](ADR-079-verifiable-compute.md), [ADR-044](ADR-044-mining-rewards-lane.md), [ADR-077](ADR-077-sentinel-recovery-network.md), [ADR-028](ADR-028-staking.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 11, 13, 17, 20 (retired), 24, 25, 26, 28, 33

## Context

ADR-079 decided the shape: a job is a content-addressed spec, payment is
escrowed and released by a verdict, the verdict ladder starts at
redundant execution (rung a), AI is advisory and never on the consensus
path, and no model runs in a node. It wrote no activation height and
built nothing; the CATALOG queue calls this "ADR exists, no job". The
owner's standing position (memory `identity-ai-native`): models never on
consensus, claims need evidence.

**Findings that contradict the brief.**

1. **`WorkClaim` has no `kind` byte.** ADR-044 says "`WorkClaim` gets a
   `kind` byte from day one (`0 = DAG PoW`)" (ADR-044 line 96) and
   ADR-079 §2 builds on `kind = 1`. The struct is `WorkClaim {
   beneficiary: Address }` and its encoding is the 32-byte address and
   nothing else (`crates/node/src/core/governance_payload.rs:120-123`,
   `:168-180`). There is no kind to set. Adding one is a wire change to a
   frozen kind, so this ADR uses a **new `TxKind`** instead and leaves
   `WorkClaim` as it is.
2. **`ClaimWork` is refused at every height** (`WORK_CLAIM_ACTIVATION_HEIGHT
   = u64::MAX`, `state/context.rs:131`; `NodeError::WorkClaimInactive`,
   `error.rs:419`). That is the ADR-079 gate and it stays.
3. **Inference is not deterministic across hardware**, so rung (a)
   "k workers agree on the output commitment" holds for a job class only
   when its runtime is fixed to one deterministic configuration. EigenAI
   pins one GPU architecture and a containerized runtime to get
   byte-equality disputes; Gensyn's RepOps make operators bitwise
   reproducible (prior-art file). A job that does not declare such a
   runtime cannot be verified by agreement, and a receipt for it is a
   signed claim, not a verdict. The ADR separates the two.
4. `crates/confidential-ai` is off-chain federated training (RESEARCH);
   nothing in it produces or checks a receipt.

## Decision

**Four records under a new prefix `cj:` implement ADR-079's job, escrow,
assignment and verdict for rung (a). A new `TxKind::ClaimCompute { job,
output_commitment }` is the claim. An inference receipt is an off-chain
signed document whose hash is the output commitment; the chain checks
byte-equality of commitments across `k` workers and never reads the
receipt's contents. Models, weights and runtimes stay off every node.**

### Records (invariant 25), layer `StateLayer::Compute`, breaker `Module::Compute`

- `cj:j:` ‖ `job_id(32)` → `Job { requester, class: u16, runtime_id(32),
  input_commitment(32), output_format: u8, resource_bound: { heights:
  u64, memory_mib: u64, flop_budget: U256 }, k: u8, rung: u8, price:
  U256, deadline: u64 }`. `job_id = blake3("maya2c job v1" ‖ canonical
  encoding)`. `rung` is `0` (agreement) in this ADR; other values are
  refused until their ADRs (079 R3, R5).
- `cj:e:` ‖ `job_id` → `U256` escrow, moved from the requester at
  creation (`checked_sub`), released by a verdict and by nothing else.
- `cj:a:` ‖ `job_id` ‖ `worker` → `Assignment { bond: U256, claimed_at:
  u64 }`. A worker self-assigns by posting the job class's bond (a
  governed value under invariant 17, read from state). **Superseded by
  the 2026-11-19 amendment below:** slots are drawn, not self-chosen.
- `cj:c:` ‖ `job_id` ‖ `worker` → `output_commitment(32)`, written by
  `ClaimCompute`.
- `cj:v:` ‖ `job_id` → `Verdict { commitment(32), agreed: u8, height }`,
  written by the end-of-block settle when `k` claims exist or the
  deadline passes.

### The settle rule (end of block, after timers, key order)

For each job whose `k`-th claim landed this block or whose `deadline` is
this height:

- If at least `⌈2k/3⌉` claims carry the same commitment: that is the
  verdict; escrow splits equally among the agreeing workers
  (`price / agreeing`, remainder to the treasury account of ADR-111, so
  no coin is created or lost); disagreeing workers' bonds are slashed to
  the treasury (ADR-028's evidence shape; bond amounts governed, never
  constants).
- Otherwise: no verdict; escrow returns to the requester; every bond
  returns. A job with fewer than `k` claims at the deadline is the same
  case.

Nothing here runs a model, reads a float or consults a clock. The
chain's whole contribution is "did `k` independent parties, each with
money at stake, produce the same 32 bytes" (invariant 24; ADR-079 §1).

### `TxKind::ClaimCompute`

New tag (the next free `TAG_*` after 53, `payload.rs:177-181`), payload
`job_id(32) ‖ output_commitment(32)`. Refused unless `cj:a:` holds the
sender's assignment and the height is before the deadline. Refused at
every height before the feature height, as `ClaimWork` is. **No mint**:
payment is the requester's escrow (ADR-044's no-mint rule kept, as ADR-079
§2 requires).

### Inference receipts (off-chain, W3)

A receipt is the worker's signed statement of what it ran:

```
Receipt v1 {
  job_id, runtime_id, model_commitment(32), input_commitment(32),
  output(bytes or its hash), worker, height_seen: u64,
  signature over blake3("maya2c inference receipt v1" ‖ all of the above)
}
```

`output_commitment = blake3(canonical receipt without the signature)`.
Two honest workers on the same deterministic runtime produce the same
receipt body and therefore the same commitment; that is what the settle
rule compares. The receipt lives with the requester, an indexer, or
ADR-119's data carrier if someone wants it public; the chain holds the
32 bytes. `runtime_id` names a published runtime manifest (container
digest, GPU class, library versions) the job class requires; a worker on
a different runtime gets a different commitment and loses its bond,
which is the incentive to run what the spec says.

A receipt for a job class with **no** deterministic runtime (`k = 1`,
or a class flagged `advisory`) is a claim with a signature and no
verdict. The chain refuses to escrow for it (`k >= 2` is required for
rung 0), so such receipts are reputation data for off-chain readers and
never trigger a payment. This is the line between "verified" and
"attested" that the prior-art survey says vendors blur.

### AI proposes, never on consensus

- No node parses a model, a prompt or an output. The VM has no inference
  host function and this ADR adds none.
- Worker software is off the node path (`bins/maya2c-worker`, Rust, no
  Python on a node; ADR-079 §6). Its output is a `ClaimCompute`
  transaction like any wallet's.
- Where the chain uses an AI result at all (ADR-077, ADR-079 §1), it is
  a proposal through governance with an activation height; a verdict
  here releases escrow and changes nothing else.

### Amendment 2026-11-19: drawn assignment, commit-reveal, no-reveal forfeit

The agreement rule above only means something if the `k` workers are
independent and each one ran the job. Two properties of the design as
first written do not give that, and both must change before any code:

- **Self-assignment.** If a worker picks its own slots, one party
  holding `⌈2k/3⌉` bonds under different addresses decides the verdict
  alone, and its bonds come back because it is the majority. Raising `k`
  does not help when the slots are chosen by the party.
- **Cleartext claims.** `ClaimCompute { job, output_commitment }` is
  public in the mempool and in blocks before the deadline, so a worker
  can submit a commitment it did not compute.

The rule therefore becomes:

1. **Eligibility.** An opt-in record with at least the class's governed
   bond (invariant 17). Each eligible worker has a **weight**: its
   bonded amount, and, once the share-lane redesign gives shares a
   per-miner ticket count (an open genesis-format decision), its ticket
   weight as well. Weight is never "one per address", so splitting a bond
   across addresses does not raise its weight. It does let one party
   fill more than one slot (step 2 draws without replacement, so one
   address holds at most one); the odds under **Size of `k`** already
   assume a party that has split its bond.
2. **Draw.** For a job created in DAG epoch `e` (ADR-044), slots are drawn
   when epoch `e+1` closes, or at the later epoch the job names:
   `r_s = blake3_derive("maya2c compute assign v1", anchor ‖ job_id ‖ s)`
   for each slot `s < k`; the worker whose cumulative weight interval
   holds `r_s mod W` takes the slot. Workers are walked in key order (every
   node must draw the same slots for invariant 24's state root to agree, so
   no `HashMap` iteration order), drawn without replacement, integer
   arithmetic only. `cj:a:` records are written by the draw, never by the
   worker.
3. **Commit.** Until `commit_deadline`, each drawn worker submits
   `ClaimCompute { job, commit }` with
   `commit = blake3_derive("maya2c compute commit v1", job_id ‖ worker ‖ output_commitment ‖ salt)`.
   The commit is stored under a sixth record, `cj:k:` ‖ `job_id` ‖
   `worker` → `commit(32)`. It binds the worker and a 32-byte secret
   salt, so copying one is useless. The payload stays 64 bytes
   (invariant 33).
4. **Reveal.** Between `commit_deadline` and `reveal_deadline` (the old
   `deadline`), a new `TxKind::RevealCompute { job, output_commitment,
   salt }` (96 bytes) opens the commit; the chain recomputes it and
   writes `cj:c:` only on a match. A copied value revealed after the
   commit window closed has no commit to open.
5. **No-reveal forfeit.** A drawn worker who committed and does not
   reveal by `reveal_deadline` forfeits a governed fraction of its bond
   to the treasury. Without it, "commit, then reveal only if the others
   agree" is a free option.
6. **Settle.** The `⌈2k/3⌉` rule above, over revealed commitments only.

**Size of `k`.** With drawn slots, a party holding a fraction `f` of
eligible weight decides a `k = 5` job (needs 4) with probability about
`5f⁴(1−f) + f⁵`: 0.0067 at `f = 0.2`, 0.044 at `f = 0.33` (large
eligible set; drawing without replacement makes it slightly lower). For
`k = 3` (needs 2) it is about 0.104 at `f = 0.2`. `k = 3` is therefore a
floor for cheap classes only; a class's minimum `k` is a governed value,
and high-value classes start at 5.

**Anchor influence (known, bounded, not removed).** ADR-044 records that
the proposer of an epoch's boundary block learns the next anchor first.
Once the anchor seeds the draw, that proposer can try alternative
orderings of one block to move the draw. The bound: only that one
proposer, only one block's reordering, and only for jobs whose draw uses
that anchor. A requester of a high-value job can require the draw to
hash the anchors of two consecutive epochs together, so steering it
needs both boundary proposers, and governance can raise the class's `k`. A consensus randomness beacon
would remove this; ADR-115's beacon sits under the optional oracle,
which is absent by default (invariant 11), so it is not relied on here.

## Alternatives considered

- **Add a `kind` byte to `WorkClaim`.** A wire change to a frozen kind
  used by the reward lane; a new kind is cheaper to reason about and
  keeps `ClaimWork`'s refusal untouched.
- **Optimistic single worker with challenge (ADR-079 rung b).** R3, its
  own ADR; the dispute window and challenge bond need the R2 records
  first, which are these.
- **zk proofs of inference.** ADR-079 R5; the survey finds them
  prohibitively expensive for large models today.
- **TEE receipts (Ritual's stated route).** ADR-079's invariant-11
  conflict is unresolved; rung (d) stays unused.
- **Quality scoring by validators (Bittensor's shape).** Puts a judgment
  on the payout path; the conflict of interest is noted in the prior-art
  file. Rejected.
- **Pay from block rewards.** ADR-044's lane mints for PoW shares; this
  pays from escrow. No mint.

## Consequences

- Six records (`cj:k:` from the amendment), two kinds (`ClaimCompute`, and `RevealCompute` from the
  2026-11-19 amendment), one draw and one settle pass, one breaker
  module. No change to `WorkClaim` or the reward lane.
- Escrow and bonds are 256-bit amounts with `checked_*` arithmetic and
  no 512-bit intermediate (`price / agreeing` is a U256 division).
- The chain can pay for deterministic jobs; non-deterministic inference
  gets receipts and reputation, not money, until a stronger rung exists.
- A job class's runtime manifest is an off-chain document the spec
  names by hash; publishing manifests is ops work.

## Invariants touched

- **11**: no trusted party; workers are bonded strangers.
- **13**: a job spec is data; a runtime is named by hash and never
  fetched by a node.
- **17**: bond amounts governed, read from state.
- **20** (retired, still the rule): no float on the consensus path; the
  settle compares bytes.
- **24**, **26**: verdicts and escrow moves are in the block's batch and
  under the root.
- **25**: `cj:` layer, undo covered.
- **28**: `Module::Compute`; a tripped breaker admits no job and settles
  none (escrows wait, nothing is lost).
- **33**: `ClaimCompute` is 64 bytes of payload, `RevealCompute` 96.

## Activation

`FeatureSet::COMPUTE_JOBS = 1 << 35` (claimed in `adr-claims.txt`),
compiled `COMPUTE_JOBS_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 — **not with the v4 bundle**: ADR-079 requires its own R2
decision per network and a 24-hour testnet run before any height is
written. Receipts, the worker binary and runtime manifests are W3 and
need no height.

## Test plan

Red tests, `crates/node/tests/compute_jobs_tests.rs` and
`bins/maya2c-worker/tests/receipt_tests.rs`:

- `a_job_escrows_the_price_and_refuses_a_requester_who_cannot_pay`
- `k_agreeing_claims_release_the_escrow_split_with_no_coin_created_or_lost`
- `a_dissenting_worker_loses_its_bond_to_the_treasury`
- `fewer_than_k_claims_at_the_deadline_return_escrow_and_bonds`
- `a_claim_without_an_assignment_is_refused`
- `a_claim_after_the_deadline_is_refused`
- `rung_other_than_zero_is_refused_at_creation`
- `k_below_two_is_refused_at_creation`
- `two_receipts_from_one_runtime_manifest_hash_to_one_commitment`
- `a_receipt_with_a_different_runtime_id_hashes_differently`
- `before_the_height_claim_compute_is_refused_and_claim_work_still_is`
- `a_tripped_compute_breaker_admits_no_job`
- `a_reorg_restores_escrow_assignments_and_verdicts`
- `exploit_replays::k_colluding_workers_still_need_two_thirds_and_lose_bonds_when_caught`
  — stated as what it is: collusion among `⌈2k/3⌉` drawn workers pays;
  the defence is the drawn assignment, the class's minimum `k` and the
  bond level.
- Amendment 2026-11-19:
  - `a_worker_cannot_write_its_own_assignment`
  - `splitting_a_bond_across_addresses_does_not_raise_draw_weight`
  - `a_split_bond_captures_slots_at_the_rate_its_total_weight_predicts`
    (slot capture by a split party matches the **Size of `k`** odds, not more)
  - `the_draw_is_identical_on_two_nodes_and_uses_no_hashmap_order`
  - `a_copied_commit_from_another_worker_never_opens`
  - `a_reveal_that_does_not_match_its_commit_is_refused`
  - `a_reveal_without_a_commit_is_refused`
  - `a_drawn_worker_who_does_not_reveal_forfeits_the_governed_fraction`
  - `settle_counts_only_revealed_commitments`

Spec: a compute section with vectors CJ-1..CJ-4; `cargo xtask
spec-coverage` lists them. Determinism evidence per runtime manifest
(same job, two machines, identical commitment) goes to `reports/` before
a class is enabled (ADR-079 R2's check).

## Open questions for Eric (economics only)

1. Bond level per job class (a governed value; the default to seed).
2. Whether the treasury keeps the slashed bonds and division remainders
   or burns them.
3. Whether a protocol fee is taken from escrow on settlement (today
   none; the requester pays the price, workers receive all of it).
4. The no-reveal forfeit fraction and the minimum `k` per class
   (amendment 2026-11-19; both governed values, defaults to seed).
5. Whether share tickets join bond in the draw weight; that waits on
   the share-format decision for genesis.
