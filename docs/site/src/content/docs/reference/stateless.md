---
title: 'Stateless transfer verification'
editUrl: false
# GENERATED from docs/stateless.md by scripts/ingest.mjs. Edit the source, not this.
---
**Status: RESEARCH.** `STATELESS_ACTIVATION_HEIGHT = u64::MAX`
(`crates/node/src/state/context.rs`). Nothing on any network commits accounts as a sparse
tree, and no gossip topic carries a witness.

Crates and paths: `stateless-core` (the tree, witnesses, transfer rules, Ring-SIS
backend), `crates/node/src/state/stateless.rs` (the switch, witness production,
`verify_block`), `crates/light-client/src/stateless.rs` (`StatelessValidator`), RPC
`stateless_transaction_witness`.

## The brief, and what came of each part

| Asked | Built | Met? |
|---|---|---|
| Polynomial vector commitments over `R_q = Z_q[x]/(x^n+1)` | `stateless_core::lattice::RingSis`: a Ring-SIS compression function over `n = 256`, `q = 12289`, driving the same sparse tree as the BLAKE3 backend | Built, dark, **not** what the node commits with — see below |
| A witness of under 1 KB on every transaction broadcast | `StateDB::transaction_witness`, RPC `stateless_transaction_witness` | **Per key, yes** with BLAKE3 up to ~2^20 accounts. **Per transfer, no**: a transfer reads at least two keys, ~1.3 KB at 2^18 accounts. With Ring-SIS, no at any useful size. Not on gossip |
| Validators check state purely from the header root and a witness | `state::stateless::verify_block`, `StatelessValidator`, `LightClient::verify_block` | For **transfer-only blocks** on a state with no `m:`/`o:`/`g:` records. Anything else answers *unverifiable* |
| A light node validates 10,000 transitions in under 10 MB | `crates/stateless-core/tests/light_node_memory.rs` (unsigned), `crates/light-client/tests/stateless_memory.rs` (hybrid-signed, through the node's types) | Yes, as peak *heap* above baseline — not RSS |

## Why the node does not commit with the lattice backend

The state root was already post-quantum: `state::merkle` is BLAKE3, and a hash
tree needs only collision resistance, which Grover and BHT reduce on paper to
about `2^85` quantum queries at a memory cost that leaves classical search
cheaper. A lattice commitment buys algebraic structure, not security.

And it costs size. A Ring-SIS digest is one packed element of `R_q`:
`256 × 14 = 3584` bits, 448 bytes, against BLAKE3's 32. An opening carries one
sibling per level:

| Backend | Node on the wire | One key at 2^18 accounts | Test |
|---|---|---|---|
| BLAKE3 | 33 bytes | < 1,024 bytes | `tree_tests::a_blake3_opening_of_one_key_is_under_a_kilobyte_at_a_quarter_million_accounts` |
| BLAKE3, two keys (a transfer) | — | 1,024–1,535 bytes | same test |
| Ring-SIS | 449 bytes | > 2,000 bytes at **64** accounts | `tree_tests::a_ring_sis_opening_misses_the_kilobyte_target` |

Succinct lattice vector commitments exist, and none rescue the target. A
literature pass on 2026-09-14 (sources below; figures as the papers report
them, not re-derived here) found:

- Peikert–Pepin–Sharp (TCC 2021): SIS, but a trapdoor setup — someone holds a
  key that forges openings.
- Wee–Wu (Eurocrypt 2023): succinct openings, with functional variants under
  the non-standard BASIS assumption. Wee–Wu (Asiacrypt 2023) then broke the
  *extractability* of knowledge-k-R-ISIS, the assumption behind
  Albrecht–Cini–Lai–Malavolta–Thyagarajan (Crypto 2022).
- SLAP (Eurocrypt 2024) and the Crypto 2024 polynomial commitments: transparent
  setup, standard assumptions, openings from **a few kilobytes**
  (designated-verifier) to **~225 KB** (public verification at 2^20).

No transparent, standard-assumption lattice commitment opening in under 1 KB at
128-bit security was found. So the lattice backend ships as research, measured,
and the node commits with BLAKE3.

**The Ring-SIS parameters have had no lattice estimator run on them.** SWIFFT's
published set (`n = 64`, `p = 257`, `m = 16`) does not compress two of its own
digests into one, so it cannot build a tree. `n = 256`, `q = 12289`, 28 binary
columns is exact 2:1 compression and nothing more has been argued for it. The
function is also linear — `f(x) + f(y) = f(x + y)` for disjoint supports — which
a Merkle tree tolerates and nothing else should.

## The tree

`crates/node/src/state/merkle.rs` orders accounts by address and packs them densely.
Inserting one account moves every account after it, so no witness smaller than
the whole account set can say what the new root is — and a transfer to a new
address inserts one. Stateless execution needed a different tree, which makes it
a state-root format change.

`stateless_core::sparse` places a leaf at the position its key's bits spell,
collapsing any subtree of one leaf to that leaf and any subtree of none to the
empty digest. The shape is a function of the key set, a path is about `log2 N`
deep, and an insert changes only its own path. Absence is provable: the path
ends at the empty digest or at a different leaf.

A witness is that tree with every subtree off the opened paths replaced by its
digest, in preorder (`TAG_EMPTY`, `TAG_OPAQUE` + digest, `TAG_LEAF` + key +
value, `TAG_INTERNAL`). The decoder refuses every non-canonical spelling: a
leaf outside its subtree, a split holding at most one leaf, an opaque empty
subtree, a split below bit 255, trailing bytes. It also caps bytes
(`MAX_WITNESS_BYTES`, 2 MiB) and *nodes* (`MAX_WITNESS_NODES`), because an empty
subtree is one byte on the wire and a 56-byte node in memory.

A tree is read only after `PartialTree::verify` has matched its digest to a
committed root — `VerifiedTree` is the only type that executes a transfer. A
violation computed from an unchecked witness would be a relay's choice.

## The switch

From the activation height, `stage_block` writes `sl:accounts-sparse`. That
record sits under its own layer (`StateLayer::Stateless`, tag 12, between
`Htlc` and `Shielded`; it moves no existing root because no existing state holds
it). `root_with_overlay` builds the sparse accounts root whenever the marker is
present, so `state_root()` needs no height, and the marker is journalled and
reverted like any record — `crates/node/tests/stateless_equivalence.rs` reverts the
activation block and gets the dense root back. `account_proof` refuses a sparse
state rather than hand out a dense path that verifies against nothing.

The activation block's own pre-state is dense. The first block a stateless node
can check is the one after it.

## What a witness can decide

Only a block whose transactions are all `TxKind::Transfer`, on a parent state
with no sealed, oracle or governance layer. This is the list of everything
`stage_block` does besides staging transactions, and when each is inert:

| Pass | Reads | Inert when |
|---|---|---|
| `settle_sealed` | envelopes due at this height (`m:`) | the sealed layer is absent |
| `settle_trading` | pairs the block's DEX transactions touched | no DEX transaction |
| `settle_oracle` | the authority registry (`o:`) | the oracle layer is absent |
| `settle_governance` | every proposal (`g:`) | the governance layer is absent |
| `check_anomalies` | pool records and the shielded pool the block wrote | no DEX or shielded transaction |

Transfers conserve by construction and map to no breaker module (invariant 28).
The honest consequence: a chain running governance cannot be followed
statelessly at all until the governance pass can be witnessed. That is the
largest open item.

## Invalid and unverifiable

`StatelessError::Invalid` is a verdict every full node shares: a bad signature,
a wrong nonce, an insufficient balance, an overflow, or a declared state root
the transfers do not produce. `Unverifiable` is not a verdict: a witness that
does not match the parent root, does not open a key the block reads, or a body
that fails its header's `tx_root`. The witness travels outside the block id and
the proof of work, and a relay can substitute a body under an honest header —
the censorship case invariant 24 closed. A node that marked a block invalid for
either would hand one relay a veto.

Tests: `stateless_equivalence.rs` — the full node and `verify_block` agree on
every transfer block built; broken transfers are invalid both ways; a lying
root is invalid; a substituted body, a stale witness, a foreign witness, a
pass layer and a dense parent are all unverifiable.

## Memory

Both tests count heap through a global allocator and report the peak **above
what was live when validation began**. The full-state reference that built the
chain writes frames to a file and is dropped first; the validator reads one
block and its witness at a time and keeps a 32-byte root between them. Not
counted: the binary, stacks, allocator slack, and RocksDB's C++ heap, which
only setup touches. Proof of work is not checked — `HeaderChain` does that, and
its DAG cache alone exceeds the budget.

The signed test builds 100 blocks of 100 hybrid-signed transfers from 100
senders against ~3,500 accounts, a quarter paying addresses that do not exist
yet. Signing is parallelised; SLH-DSA signing is ~140 ms a signature, and the
whole test takes about four minutes on the development machine, so it is
`#[ignore]`d. Run it with
`cargo nextest run -p maya-light-client --run-ignored all`. The unsigned
crate-level test makes the same claim in under a second and runs by default.

## Before anyone picks an activation height

1. **Witness transport.** No gossip topic carries a witness. A block witness
   has to reach stateless peers beside the block, and its size — about
   `2 × 34 × log2 N` bytes per transfer — belongs in the block-size budget.
2. **Witnessable end-of-block passes,** or stateless nodes stop at the first
   governance proposal. The records that decide whether a pass fires (the due
   envelope range, the proposal queue head) would have to be opened in the
   witness and the passes re-run against it.
3. **Mempool staleness.** A transaction witness is stale after one block,
   because every block changes the top of every path. A relay has to refresh
   it or drop it — never treat it as an invalid transaction.
4. **The lattice parameters,** if the research backend is ever to be more than
   a size comparison: an estimator run, and a reason to prefer it to a hash.
5. **Root cost.** A full node rebuilds the sparse root from every account each
   block, as it already does for the dense tree. Both are `O(N)`; an
   incremental tree is a separate change.
6. **The RPC method's cost.** `stateless_transaction_witness` scans every
   account per call and does not verify the transaction it is handed, the same
   cost `account_proof` already has. Dormant today — before activation it reads
   one record and refuses — but on activation day it wants a rate limit, or a
   signature check before the scan.

## Sources

- C. Peikert, Z. Pepin, C. Sharp, *Vector and Functional Commitments from
  Lattices*, TCC 2021 — ePrint 2021/1254.
- H. Wee, D. J. Wu, *Succinct Vector, Polynomial, and Functional Commitments
  from Lattices*, Eurocrypt 2023 — ePrint 2022/1515.
- H. Wee, D. J. Wu, *Lattice-Based Functional Commitments: Fast Verification
  and Cryptanalysis*, Asiacrypt 2023 — ePrint 2024/028.
- M. R. Albrecht, V. Cini, R. W. F. Lai, G. Malavolta, S. A. K. Thyagarajan,
  *Lattice-Based SNARKs: Publicly Verifiable, Preprocessing, and Recursively
  Composable*, Crypto 2022.
- *SLAP: Succinct Lattice-Based Polynomial Commitments from Standard
  Assumptions*, Eurocrypt 2024 — ePrint 2023/1469.
- *Polynomial Commitments from Lattices: Post-Quantum Security, Fast
  Verification and Transparent Setup*, Crypto 2024 — ePrint 2024/281.
- V. Lyubashevsky, D. Micciancio, C. Peikert, A. Rosen, *SWIFFT: A Modest
  Proposal for FFT Hashing*, FSE 2008.
- D. J. Bernstein, *Cost analysis of hash collisions: Will quantum computers
  make SHARCS obsolete?*, SHARCS 2009.
