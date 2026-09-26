---
title: 'Node types and what they need'
editUrl: false
# GENERATED from docs/NODE_TYPES.md by scripts/ingest.mjs. Edit the source, not this.
---
Master Prompt 14 §1. Every figure is either **measured** on the machine in
`reports/12-baseline.md` (4 vCPU Xeon @ 2.8 GHz, 16 GiB, virtio disk) or
**derived** by arithmetic from a measured figure, and says which. Nothing
here is an estimate without a source.

## Measured building blocks

| Quantity | Value | Source |
|---|---|---|
| Hybrid transaction on the wire | 13,215 B | `apply_pipeline`, 400 real transfers |
| Hybrid verification | 1,010–1,082 /s/core; 3,086 /s on 4 cores | `apply_pipeline`, `certificate` bench |
| Block application incl. verify | 942 tx/s on one core | `apply_pipeline`, optimized |
| Idle node process | 80.3 MiB RSS, 75 MB data dir at height 0 | `docker stats`, local-docker node |
| Vote certificate, n = 100 | 216.5 KiB; 7.95 ms verify on 4 cores | `certificate` bench |
| RPC reads, one node | 21,231 req/s (16 clients), p99 1.6 ms | `maya2c-rpc-load`, mixed reads |
| Snapshot sync, 100M accounts | 4.80 GB; joiner verify 2.4 s CPU | `state-sync-sim` (virtual network) |
| Header | 144 B | `spec/05-consensus.md` CON-1 |

Disk growth per month for transaction bodies, **derived**
(13,215 B × TPS × 2,592,000 s):

| Sustained TPS | Per month |
|---|---|
| 10 | 343 GB |
| 100 | 3.43 TB |
| 1,000 | 34.3 TB |

## The types

| Type | Stores | Verifies | Minimum (derived at 100 TPS) | Recommended |
|---|---|---|---|---|
| **Validator** | state, recent blocks, undo journals to the prune horizon | every transaction, every certificate | 4 cores, 8 GiB, 1 Gbit/s, 4 TB/month if unpruned | 8 cores, 16 GiB, NVMe, pruning on |
| **Full node** | state, blocks since the prune horizon | every transaction | 2 cores, 4 GiB, 100 Mbit/s | 4 cores, 8 GiB |
| **Archive node** | every block, signature and journal forever | every transaction | full node + 3.4 TB/month at 100 TPS | object storage behind it |
| **RPC node** | a full node's state | as a full node | full node sizing; ~21k simple reads/s per node process on 4 vCPU | several behind the gateway; heavy history on the indexer |
| **Light client** | headers only: 144 B each (0.83 MB/day at 15 s blocks) | headers, PoW, Merkle proofs; trusts no server | a phone or a browser tab | — |
| **Compute worker** | — | — | deferred (not built) | — |

How the validator row is derived at 100 TPS:

- verification is 100 of ~1,000 per core per second;
- certificates at n = 100 and 1 s rounds cost 1.2 cores (ADR-021);
- 13,215 B × 100 = 1.3 MB/s ingest, plus 177 Mbit/s of certificates.

## Not measured

- Validator memory under load, and a real node's disk growth with pruning on.
- Bandwidth with gossip duplication.
- RPC capacity with a large state: the curve above ran against an empty
  chain, so reads never missed the block cache.
- Light-client sync time from genesis. The recursive "chain valid to epoch E"
  STARK is not built.
