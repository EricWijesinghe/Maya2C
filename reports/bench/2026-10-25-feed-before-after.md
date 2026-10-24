# Feed change: before and after (finalized transfers/s)

`cargo xtask bench-tps` (ADR-059), 4 validators on localhost, `--profile perf`
binaries, one run per cell, **machine shared with the live testnet's PC
validators** (absolute figures are shared-machine figures). All transfers
accepted were finalized and visible on a replica that never received them, in
every cell.

| senders x txs | before (master f60b3f9a) | + frame channel (#173) | + feed change (this PR) |
|---|---|---|---|
| 8 x 10 | 6.9 | n/m | **19.1** (4 blocks) |
| 32 x 40 | 32.5 | n/m | **164.9** (8 blocks) |
| 64 x 60 | 9.4 | n/m | **294.3** (10 blocks) |
| 1000 x 4 | 153.2 | n/m | **366.5** (9 blocks) |
| 3000 x 2 | 12.8 (470 s stall, 4,016 lost frames) | 97.8 | **449.7** (10 blocks, 13.3 s) |

n/m = not measured at that build.

What changed (`bins/maya2c-node/src/bft.rs`, `network/mempool.rs`; node policy,
not consensus):
1. The BFT feed no longer clones and re-hashes the whole mempool every 100 ms
   tick (`Mempool::scan`). Block time used to grow with the pool.
2. A sender's transactions go to one proposer, in nonce order. Split by txid,
   an account's consecutive nonces executed out of order, about one per block,
   which capped any single account near one transfer per block.

Not shown: an idle machine, WAN latency, separate machines, contract calls or
mixed loads, sustained runs, 21+ validators, and whether any other part of the
node limits at higher load. 449.7/s is one point on one shared machine, not a
ceiling and not a network figure. Single runs: repeat before quoting.

## Second run, after the review follow-up (sender cache, sorted unannounced)
Same machine, same settings, different binary (`perf/feed-nonce-order` tip):
64 x 60: **374.1/s** (3,840 in 10.3 s, 7 blocks); 3000 x 2: **341.9/s** (6,000
in 17.6 s, 7 blocks). Against the first run (294.3 and 449.7) that is +27% and
-24% on a machine shared with the live testnet: **single runs vary by about
+/-25%**, so only differences of several times, not tens of percent, are
supported by this data. Repeat on an idle machine before quoting a figure.

## Ceiling probe: 6000 senders x 2 (12,000 transfers)
444.9/s (12,000 finalized and visible on the replica in 27.0 s over 13 blocks,
about 920 per block; submitting took 1.0 s). Together with the 3000 x 2 runs
(449.7 and 341.9) that is a plateau of roughly 350 to 450 per second on this
shared machine: about 2.2 ms per transfer end to end across four validators
that all verify and execute every transfer.
It is **not** signature cost: recorded hybrid verification is ~0.21 ms
(`reports/crypto-bench.md`). What the remaining time is (state commit, execution,
gossip of 13 KB transactions to three peers, CPU shared with eight other
validators) has not been profiled. That is the next measurement; no number here
supports a claim about where the limit is.
