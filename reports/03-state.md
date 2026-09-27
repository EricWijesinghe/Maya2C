# 03 — State, storage and economics

Master Prompt 3's DONE WHEN, item by item, with the command that produced each
figure. Where a criterion is not met, it says so.

**Machine (every figure below):** cloud VM, 4 × Intel Xeon @ 2.80 GHz (1
thread/core), 15 GiB RAM, Ubuntu 24.04.4, kernel 6.18.44, `rustc 1.99.0-nightly
(da80ed070 2026-07-14)` = the pinned `nightly-2026-07-15`. Tests ran with
`--profile ci` (dev + no debug info) unless stated.

## Verdict

| DONE WHEN criterion | State | Evidence |
|---|---|---|
| Core state tests and property tests pass | **Met** | §1 |
| Fee market is active | **Met** (2026-09-27, on any genesis with `bft.fees`) | §7 — ADR-029; `tests/fee_market_live_tests.rs`; devnet run below |
| Pruned-node test passes | **Met** (disk saving not measured) | §3 |
| DNA round-trip passes under noise | **Met** | §4 |
| `reports/03-state.md` has real numbers | this file | — |
| Recursive history compaction (§6) | **Accumulator built; validity folding not started** | §5 |

## 1. State tests (full workspace run, 2026-09-27, `cargo test --workspace --profile ci --no-fail-fast`)

```
tests/state_tests.rs            :: test result: ok. 19 passed; 0 failed; 0 ignored; ... in 5.05s
tests/stateless_equivalence.rs  :: test result: ok. 11 passed; 0 failed; 0 ignored; ... in 7.60s
tests/light_node_memory.rs      :: test result: ok. 1 passed; 0 failed; 0 ignored; ... in 1.10s
tests/exploit_replays.rs        :: test result: ok. 11 passed; 0 failed; 0 ignored; ... in 15.79s
tests/fee_market_tests.rs       :: test result: ok. 6 passed; 0 failed; 0 ignored; ... in 0.98s
tests/treasury_tests.rs         :: test result: ok. 10 passed; 0 failed; 0 ignored; ... in 0.00s
```

Whole workspace in that run: **234 test binaries, 2,626 passed, 0 failed, 6
ignored**, 55 min 50 s wall (raw log: `reports/raw-test-baseline-2026-09-27.log`).
The six ignored tests each state why (hour-long or needing a kubo daemon).

**Property test added by this work:** `crates/node/tests/supply_property_tests.rs`
— 30 seeded random blocks of real hybrid-signed transfers mixing overdrafts,
replayed and skipped nonces and self-transfers; after every block total supply
is unchanged, and every refused block leaves the state root untouched. Output
is in §6 (run after the node rebuild).

Supply conservation is also *proved* for a model of the transfer transition:
`formal/lean/Maya2C/Supply.lean`, theorem `transfer_conserves` (Master
Prompt 8, `reports/08-security.md`).

## 2. Fee market — not active

The brief requires the fee market active by default at the mempool and in the
state machine. It is not: `crates/fee-market` implements EIP-1559 base fee,
80/20 burn/treasury split and the supply cap, verified (Kani, Lean vectors,
Z3), but `FeeConfig::DISABLED` (activation `u64::MAX`) is what every network
runs, and the node's transactions carry no fee fields. Activating it is a
consensus change (new wire version, fee fields, mempool gate, reward engine)
that needs a spec section and conformance vectors first (Master Prompt 15);
ADR-016 lists it as launch-blocking. Multi-dimensional fees (compute, storage,
bandwidth, proof verification) are not built.

What *is* measured about it: `econ/` runs the chain's own `next_base_fee` and
`split` — a spam campaign drives the base fee from 10 to 77,609 per byte within
the week it runs, and the fee quote a wallet gets is never exceeded in 50,000
simulated blocks (`reports/18-economics.md`).

## 3. Pruning, archive, resurrection

```
tests/pruned_node_tests.rs :: test result: ok. 10 passed; 0 failed; 0 ignored; ... in 73.95s
```

Covers: a pruned node bootstraps from a snapshot without a single body below
the snapshot and validates new blocks; snapshots with a changed balance,
changed contract slot or missing code are refused; archives are written,
pruned and served back verified; a reorg below the horizon is refused.

**Not measured:** the brief's "disk saving (target 90%)" and "100% of accounts
restorable". No test measures on-disk bytes before and after pruning.
Dormant-account expiry and resurrection by inclusion proof are **not built**.

## 4. DNA archive codec

```
tests/dna_tests.rs :: test result: ok. 5 passed; 0 failed; 0 ignored; ... in 3.31s
```

`a_megabyte_survives_substitutions_indels_and_fifteen_percent_strand_loss`
is the brief's test: 1 MB → strands (rotation code, homopolymers ≤ 3, GC
balanced, inner + outer Reed–Solomon, primers, FASTA) → substitutions,
insertions, deletions and 15% strand loss → lossless restore. The lab
(synthesis, sequencing) is SIM.

## 5. History compaction (new: `crates/history-compactor`)

A Merkle Mountain Range over one leaf per block. `cargo bench -p
maya-history-compactor --bench compactor_bench` (release):

```
history-compactor bench (MMR accumulator; NOT a validity proof)
blocks                         1000000
pruned fold time               0.312 s (312 ns/block)
pruned node memory             7 peaks x 32 B = 224 B
archive build time             0.338 s
archive memory                 1999993 nodes x 32 B = 61.0 MiB
max inclusion proof            818 B
prove+verify (10000 samples)   4.6 us each
```

`tests/compaction_tests.rs` (3 tests): a pruned node holding only the
commitment verifies old transactions an untrusted archive serves; a forged
transaction, a forged block, a block moved to another height and a one-bit
proof flip are each refused.

**What it is not:** a validity proof. It proves a block is the one the chain
committed to, not that the history was valid. Nova-family folding is
curve-based (not post-quantum); a recursive Plonky3 verifier does not exist in
this tree. That half of §6 is not started, and the brief's "folding time for
1,000,000 blocks" is reported above for the accumulator only.

## 6. Not built

- Account model ADR, borsh/SSZ canonical encoding choice ADR, rkyv/flatbuffers
  zero-copy wire format — the node's hand-written canonical encoding is fuzzed
  for canonicality, but the ADRs the brief asks for were not written here.
- StateDB column families as specified (the node uses prefixes in one
  keyspace).
- Commitment trait with Verkle (IPA/KZG) and lattice backends — the stateless
  path has a sparse Merkle tree and a Ring-SIS research backend (`docs/stateless.md`).
- Tiered storage HAL (`hal/storage`: CXL, MRAM, NVMe-oF, glass SIMs) and
  `benches/cxl_bench.rs`; NUMA placement.
- Hyperdimensional index (optional in the brief).

## 7. The fee market, live (added 2026-09-27, ADR-029)

Machine for this section: Windows 11, Intel family 6 model 198 (24 threads),
dev profile. A fee is an ordinary signed output to `FEE_COLLECTOR`; the base
fee on the transaction's serialized size burns to the sink each block, the
remainder (the tip) pays validators at the staking epoch boundary.

```
$ cargo test -p custom-l1-node --test fee_market_live_tests
test the_base_fee_falls_on_empty_blocks_and_never_below_its_floor ... ok
test an_unpaid_transfer_is_refused_and_a_paid_one_burns_its_base_fee ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.36s
```

The same rules through the real binary: `scripts/bft_devnet.py` runs a genesis
with `bft.fees` and `bft.staking`; `l1-wallet send` attaches the fee output
itself (twice the base fee on the signed size). Five processes crossed two
staking epochs, every node agreed at the common height, and each transfer was
visible on all five in 1.9–3.0 s.

What stays open: the fee *parameters* are devnet values; justifying mainnet
values from measured traffic is Master Prompt 18's.
