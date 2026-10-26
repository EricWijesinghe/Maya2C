# Burst of 6,000 transfers: before and after the consensus-frame channel

`cargo xtask bench-tps --senders 3000 --txs 2` (ADR-059), same machine, same
`--profile perf` build settings, 4 validators on localhost. Shared with the
live testnet's PC validators, so absolute figures are shared-machine figures;
the comparison is the point. One run each.

| | node build | blocks | seconds | finalized/s | `lost consensus frame(s)` in node logs |
|---|---|---|---|---|---|
| before | origin/master f60b3f9a | 17 (after a long stall) | 470.2 | 12.8 | 3,941 + 55 + 20 on node 0 |
| after | fix/consensus-frame-channel 50893453 | 10 | 61.4 | 97.8 | 0 on nodes 0 and 3 |

Cause and fix: `crates/node/src/network/bus.rs`. Transaction gossip events and
consensus frames shared one 256-slot broadcast channel, so a flood of cheap
events pushed votes out while the BFT loop was busy under the chain lock.

Not shown: that no other component lags under a larger flood, that the figure
holds off a shared machine, or behaviour over a real network.
