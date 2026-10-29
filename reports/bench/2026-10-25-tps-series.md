# Finalized transfers/s, 4-validator localnet — measured series (2026-10-25)

Tool: `cargo xtask bench-tps` (ADR-059). Binaries: `--profile perf` (release
optimisation, no LTO) of `origin/master` f60b3f9a plus the harness; the raw
per-run files are the `tps-*.md` beside this one (their `commit:` line shows the
branch tip at run time, the harness itself was then uncommitted).
Machine: Windows 11, 24 logical cores, **shared with the live testnet's four PC
validators and with cargo builds** — every figure is a shared-machine figure.
Mix: single-output transfers of one base unit, hybrid ML-DSA-65 + SLH-DSA,
one sender's nonces in order, signing excluded, localhost links.

| senders x txs | total | blocks | seconds | finalized/s | what limited it |
|---|---|---|---|---|---|
| 8 x 10 | 80 | 11 | 11.6 | 6.9 | block interval (~1/s) |
| 32 x 40 | 1,280 | 38 | 39.4 | 32.5 | ~one transfer per sender per block |
| 64 x 60 | 3,840 | 72 | 408.1 | 9.4 | same, and blocks slowed to ~5.7 s |
| 1000 x 4 | 4,000 | 10 | 26.1 | **153.2** | 400 per block, ~2.6 s per block |
| 3000 x 2 | 6,000 | — | stalled | — | 0 visible after 90 s (run 1) |
| 3000 x 2 | 6,000 | 17 | 470.2 | 12.8 | recovered after a long stall (run 2) |

## Reading it honestly
1. Few senders measure the **nonce rule, not the nodes**: a block takes about
   one pending transfer per sender, so 32 senders cap near 32 per block.
2. The best figure, 153 transfers/s, is one point at 1000 senders on a shared
   machine. It is not a ceiling and not a claim about a network.
3. **A burst stalls consensus.** With 6,000 transfers arriving at once the nodes
   logged `bft: fell behind and lost N consensus frame(s)` (3,941 on node 0).
   The network's single event channel holds 256 events (`network/node.rs:34`)
   and carries transaction gossip and consensus frames together, so a flood of
   transactions can push votes out. The chain recovered on its own after ~8
   minutes (28 s per block), so it is a liveness degradation, not a halt.
   Public RPC limits (50/s per address) do not stop a distributed sender set.
4. Signature cost is not the limit: recorded hybrid verification is ~0.21 ms
   (`reports/crypto-bench.md`), thousands per second per core.

## Not measured
Contract calls, mixed loads, WAN latency, separate machines, 21 or 100
validators, sustained hours, and an idle machine. Repeat each point; this is a
single run per row.
