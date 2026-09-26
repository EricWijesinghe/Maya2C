# ADR-015: One consensus design — DAG-BFT orders, work never does

**Status:** Accepted
**Date:** 2026-09-27

## Context

Master Prompt 4 §0 asks for the design to be resolved *before* code: earlier
prompts mixed proof of work, "useful" proof of work, proof-of-stake staking and
DAG-BFT, and a chain cannot be ordered by four mechanisms at once. It asks for
a pluggable `ConsensusMode` (`argonblake-pow`, `pouw-lattice`, `dag-bft`) with
the same state machine under every mode.

What the tree has (2026-09-27):

- `custom-l1-node` orders blocks by **ArgonBlake proof of work**, most
  cumulative work wins, with reorg and state rollback (`consensus::chain`,
  `StateDB::revert_block`). That is the only mode a node can run.
- `crates/lattice-pow` verifies short-vector solutions; nothing calls it.
- `crates/blockgraph` has Narwhal-style batch references and a deterministic
  scheduler; nothing in consensus references a batch.
- No DAG commit rule, no certificates, no validator set.

## Decision

1. **Ordering and finality: DAG-BFT.** A Narwhal-style certified DAG — each
   vertex references ≥ 2f + 1 certificates of the previous round and is
   certified by 2f + 1 votes — with the **Bullshark** commit rule
   (partially-synchronous variant): even rounds have a round-robin anchor,
   committed directly on f + 1 votes from the next round, earlier anchors
   committed through paths, causal history emitted in `(round, author)` order.
   Bullshark over Tusk because its commit latency is two rounds in the common
   case rather than an expected four-and-a-half; over Mysticeti because
   Mysticeti's uncertified DAG needs a more intricate equivocation argument
   that is not worth taking on before the certified version is audited.
   Implemented sans-IO in `crates/dag-bft`.
2. **Staked validators run it.** Equal-weight committee in the engine; stake
   weighting, entry and exit are the staking module's (Master Prompt 4 §9).
3. **Work never orders anything on mainnet.** Where work enters is stated
   exactly: *nowhere in mainnet v1*. PoUW as a Sybil filter for compute
   workers or as an issuance channel is deferred until a use is measured
   (ADR-launch-scope, Master Prompt 11). A lattice puzzle whose output nobody
   uses is lattice PoW and is called that (`docs/lattice-pow.md`).
4. **Three modes, one fork-choice for the two work modes.** `argonblake-pow`
   (devnet, and what the node runs today) and `pouw-lattice` (RESEARCH)
   differ only in their work function; `maya_dag_bft::ForkChoice` is the one
   most-work engine both use in the simulator. `dag-bft` is the mainnet mode.
5. **The mode is a genesis parameter, not a `config.toml` key.** The brief
   says "set in config". `crates/node/src/config.rs` is built so that *no
   consensus value can be named in a per-host file* — a node whose file
   disagreed with its peers would fork. The mode is the most consensus-critical
   value there is, so it goes where `pow_limit` and the DAG activation height
   already go: `genesis.json`, diffed by every operator against the ceremony
   commitment. (This contradicts the brief's wording and is reported under
   Standing Order 9 rather than worked around.) `ConsensusMode::validate`
   refuses a work mode in a production build (Master Prompt 11 §3).
6. **One state machine.** Every mode emits an ordered transaction sequence;
   the state machine consumes the sequence and never learns which mode made
   it. In the simulator that is `maya_dag_bft::Ledger`; in the node it will be
   `StateDB::apply_block`.

## Consequences

- `tests/modes_sim.rs` runs all three modes under `maya-sim`: 5 validators
  over a lossy, reordering wide-area link commit prefix-consistent logs; one
  crashed validator of five does not stop commits; a 3/2 partition heals with
  the minority catching up; a 2/2 split of four (no quorum either side) halts
  rather than forks; both work modes converge six blocks deep with one state
  root.
- Every throughput figure from the engine is an **ordering** figure (no
  signature verification, execution or persistence) and is labelled so. The
  simulated cluster's ceiling is `batch_size × n / round_time`; with 500
  references per vertex and ~150 ms rounds on the modelled link that is about
  15,500 tx references per virtual second for 5 validators
  (`reports/04-consensus.md`). The 50,000 TPS goal is not measured, because the
  node does not run this engine yet.
- **Not wired into `custom-l1-node`.** What that needs, in order: ML-DSA vote
  signatures and their aggregation (Master Prompt 13); a transport binding for
  `Message` over the existing libp2p stack; epoch changes (validator-set
  rotation at epoch boundaries); a block builder that turns a committed
  sub-DAG into `apply_block` input; and a migration from the PoW chain's state
  at a written-down activation height. Until then this crate is RESEARCH.
- Vote authenticity is taken on trust inside the simulator (votes are ids,
  not signatures). The Byzantine cases tested are crash, loss, reordering,
  partition and equivocation-by-construction (one vote per slot); an actively
  lying certificate author is not modelled.

## Revisit when

- The node runs DAG-BFT on a devnet (then the throughput figure becomes TPS
  and must be re-measured under the Production Standing Orders), or
- a measured use for PoUW output appears, or
- Mysticeti-style uncertified commit is audited in a production system that
  this tree could follow.
