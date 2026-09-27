# 14 — Horizontal scale

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 14
Hardware record: [12-baseline.md](12-baseline.md).

> DONE WHEN: docs/NODE_TYPES.md has measured requirements; the 100-million-account
> state sync test passes with reported time; the lying-RPC light client test
> passes; RPC scaling curve is in reports/14-scale.md; the validator-count limit
> is measured and documented.

## 1. Node types

`docs/NODE_TYPES.md`. Every row cites a measurement or an arithmetic step
from one; the "not measured" list is part of the page.

## 2. State sync at 100 million accounts

```
$ cargo run --release -p maya-state-sync --bin state-sync-sim -- --accounts 100000000
[SIM network] state sync: 100000000 accounts (4.80 GB), 10000 chunks, 32 peers (4 malicious), 1000 Mbit/s uplink each
virtual sync time     9.4 s
downloaded            4.80 GB (1.92 MB wasted on bad chunks)
banned peers          [1, 2, 3, 4]
manifest build (CPU)  11.0 s   joiner verify (CPU) 2.4 s
balances imported     49999999950000000
real	0m36.399s
```

- Every chunk is verified against the manifest root with a Merkle proof as it
  arrives.
- All four lying peers are banned on their first bad chunk. What they still
  owed is re-queued, and the waste is 1.92 MB.
- `balances imported` is the importer's fold over every record it accepted.
  Exactly-once import is asserted by `crates/state-sync/tests/sync_tests.rs`
  at 1M accounts; the 100M run reports the sum and does not assert it.

**What "virtual" means.** The 9.4 s is the network model's clock: 32 peers at
1 Gbit/s each, and the joiner's link is not the limit. The CPU times are real.
The run is `[SIM]` and says so in its first line.

**What it is not.** It is not a join of a real node against real peers:

- the node does not yet commit a snapshot root in its header;
- warp-style replay after the snapshot is not wired;
- the node's existing `--bootstrap-from` checks a whole import against
  `state_root` instead.

## 3. Light client against a lying RPC server

`crates/light-client/tests/spv_tests.rs::the_light_client_catches_a_lying_rpc_server`:
a server returns a wrong balance, and the client rejects it by checking the
Merkle proof against a header it verified itself. Run in §6 below.

The recursive "chain valid up to epoch E" STARK is not built. Neither are
header sync across validator-set changes (there are no validator sets under
PoW) and the WASM and uniffi bindings of this client.

## 4. RPC scaling curve

`maya2c-rpc-load` (new, `crates/loadgen`, standard library only) sends mixed
reads (`get_tip_height`, `get_supply`, `get_balance`, 2:1:1) over persistent
HTTP/1.1. The targets are the local-docker nodes of `reports/10-launch.md`,
each a real `maya2c-node` process in its own container.

```
targets 1 threads 4  secs 10: 16540 req/s, 0 errors, p50 210 µs, p99 679 µs
targets 1 threads 16 secs 10: 21231 req/s, 0 errors, p50 736 µs, p99 1635 µs
targets 1 threads 64 secs 10: 21223 req/s, 0 errors, p50 2905 µs, p99 6548 µs
targets 2 threads 16 secs 10: 27223 req/s, 0 errors, p50 530 µs, p99 1807 µs
targets 4 threads 16 secs 10: 27672 req/s, 0 errors, p50 382 µs, p99 3169 µs
targets 8 threads 16 secs 10: 29601 req/s, 0 errors, p50 264 µs, p99 2651 µs
targets 8 threads 64 secs 10: 28121 req/s, 0 errors, p50 1868 µs, p99 10325 µs
```

**Reading it honestly.** Every node and the load generator share one 4-vCPU
host, so the curve flattens at about 28–30k req/s. That is the host's limit,
not the design's. One node saturates at about 21k req/s. A second node adds
28 %, because the first node was not using every core. After that, adding
nodes only moves the same CPU around, and p99 rises with contention.

What this measures is per-request server cost: roughly 21k simple reads per
second per 4-vCPU node on an empty chain. It does **not** show horizontal
scaling. That needs nodes on separate machines, which this environment does
not have.

The brief's 50,000 req/s target was **not reached** on this host. It is not
reported as reached.

Not built: per-method cost accounting and tiered API keys, and the
Postgres-indexer split for history.

## 5. Validator-count limit

Derived from measured certificate sizes and verification rates, not from a
geographic simulation. ADR-021 has the full table. Under the finality SLO
(p50 ≤ 2 s, so rounds of about 1 s), certificates alone cost:

| n | bandwidth | cores |
|---|---|---|
| 100 | 177 Mbit/s | 1.2 |
| 200 | 704 Mbit/s | 4.9 |

**Design limit: 100 validators for v1**, on a 1 Gbit/s, 4-core validator.

### 5a. Measured (2026-09-27)

`crates/node/examples/committee_scale.rs`: n real DAG-BFT engines with ML-DSA-65
authenticators, every frame encoded with the node's wire codec and delivered
to every other validator (gossip floods, so addressed votes reach everyone
and are decoded and dropped). Empty vertices: consensus overhead alone.
`--profile perf`, Windows 11, Intel family 6 model 198, one core doing each
validator's work in turn.

| n | CPU ms / round / validator | MB received / round / validator | at 1 s rounds: cores | Mbit/s |
|---|---|---|---|---|
| 4 | 4.0 | 0.06 | 0.00 | 0 |
| 16 | 32.0 | 1.18 | 0.03 | 9 |
| 32 | 101.5 | 4.77 | 0.10 | 38 |
| 64 | 351.6 | 19.56 | 0.35 | 156 |
| 100 | 844.9 | 47.97 | 0.84 | 384 |

Both grow as n²: every validator verifies and receives every other's
proposal, vote and certificate. **Bandwidth binds first**: at n = 100 a
validator receives 384 Mbit/s of consensus traffic alone — within a 1 Gbit/s
link, but at about 40 % of it before any transaction payload; n ≈ 160 would
fill it. The measured limit therefore **confirms the design limit of 100** and
says why: it is set by flooding, and the largest share is votes, which are
addressed to one author but flooded to all. Sending votes point-to-point
(a request-response protocol beside gossip) is the change that would move it.


**Not done:**

- The geographic simulation with published inter-region RTTs at
  50/100/200/400 validators. `crates/dag-bft/tests/modes_sim.rs` has a
  network model but no latency matrix, so finality p50/p99 per validator
  count was not measured.
- Sharding in production: ADR-016 keeps sharding out of v1, so there is no
  cross-shard load test.

## 6. Test runs

```
$ cargo test -p maya-light-client --profile ci --test spv_tests the_light_client_catches_a_lying_rpc_server
test the_light_client_catches_a_lying_rpc_server ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 14 filtered out; finished in 0.71s

$ cargo test -p maya-state-sync --profile ci     # liars banned, every record imported once (1M accounts)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.77s
```
