# Prior art: Inclusion lists and fair ordering

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "EIP-7547 inclusion lists FOCIL EIP-7805 fair ordering Themis Aequitas Narwhal Bullshark censorship resistance"; Aequitas/Themis and Narwhal/Bullshark did not surface in results and are cited from the papers the code already references).

Feature: parity row P-49, [ADR-124](../adr/ADR-124-inclusion-and-fair-ordering.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| EIP-7805 FOCIL (Draft) | A per-slot committee (16 validators) publishes inclusion lists the next block must honour, enforced by fork choice; no dedicated incentive; proposed for Glamsterdam/Hegota. | [EIP-7805](https://eips.ethereum.org/EIPS/eip-7805), [RIG post](https://rig.ethereum.org/post/EIP-7805-Fork-choice-enforced-Inclusion-Lists-(FOCIL)), [Ethereum Magicians](https://ethereum-magicians.org/t/eip-7805-committee-based-fork-choice-enforced-inclusion-lists-focil/21578) |
| EIP-7547 (set aside) | Single-proposer forward inclusion lists; dropped before Pectra for FOCIL. | [EthSystems pattern](https://ethsystems.org/patterns/pattern-focil-eip7805/) |
| Encrypted mempools (EIP-8184 LUCID, glossary) | Complement to inclusion lists: hide content until ordered. | [spark.money glossary](https://www.spark.money/glossary/inclusion-list) |
| Bullshark (Spiegelman et al., CCS 2022) | DAG commit rule; every validator's vertex carries its own batch; the commit order is a function of the DAG. | cited in `crates/dag-bft/src/commit.rs:1-12` (not re-searched) |
| Aequitas / Themis (order fairness) | Receive-time order across validators. | not searched; from knowledge, marked thin |

## What Maya2C has (checked in repo, 2026-11-12)

- No single proposer: `n` authors, FIFO self-packed batches (`dag-bft/src/validator.rs:97-114`), `(round, author)` commit order (`commit.rs:136-160`), fair deficit round-robin admission with a termination bound (ADR-072 CON-13).
- ADR-025: uniform-price DEX batches, optional sealed mempool, validators forbidden to sell ordering; within-batch order is the author's.

## How the Maya2C version differs

- The inclusion half is **structural**: each author's certified vertex is its inclusion list; ADR-124 adds the pinning test, not a committee.
- Ordering inside a batch becomes `blake3(anchor certificate digest ‖ sender)` then nonce: author-blind at pack time, nonce-safe, deterministic; a leader-grinding residual is stated for the two human reviewers.
- No receive-time ordering (no clock, Standing Order 4); encrypted ordering stays the optional sealed mempool.

**Verdict: similar exists (FOCIL, Bullshark's multi-proposer DAG). Differentiate by the pinned inclusion bound and the sender-keyed in-batch order. No "first" claim.**
