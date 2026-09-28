# Cross-chain: what each route trusts

Master Prompts 6 §3 and 25. Three routes exist in the tree; none is called
by consensus.

| Route | Crate | Moves | Trusts |
|---|---|---|---|
| Hash-locked swap | `htlc-watcher`, node HTLC records | value both ways, no custody | each chain's own finality; a party that stays online until its timeout |
| Bitcoin lock/mint | `btc-bridge` over `btc-spv` | BTC in as wrapped BTC; burn to request BTC out | the most-work Bitcoin chain to the stated depth, and **whoever holds the lock key** for releases |
| Ethereum light client | `interop::beacon` | proofs of Ethereum finality (no asset route yet) | the sync committee (512 validators, ≥ 2/3 signing) |

## Bitcoin: depth is a policy, not a proof

`btc-bridge` mints only for a deposit buried `min_confirmations` deep on the
most-work header chain it follows. An attacker with a share `q` of Bitcoin's
hash rate can still reverse a deposit that deep with the probability below.
It is computed from the Bitcoin whitepaper's formula (§11), and the values
match the whitepaper's own table at `q = 0.1`:

| Depth `z` | `q` = 10% | `q` = 30% |
|---|---|---|
| 1 | 0.2045873 | 0.6277491 |
| 2 | 0.0509779 | 0.4457171 |
| 3 | 0.0131722 | 0.3245841 |
| 6 | 0.0002428 | 0.1321112 |
| 10 | 0.0000012 | 0.0416605 |

The tests use 6 (`crates/btc-bridge/tests/bridge_tests.rs`), including the
brief's six-block reorg: a deposit reorganised out before it is buried
deeply enough is never credited. A deposit the bridge has already credited
and an attacker later reverses is a loss; the depth to require is a
decision made against the table above and the value at risk.

## Bitcoin: releases need a key

Bitcoin's script cannot see this chain, so no bridge can release BTC
trustlessly. A burn opens a release request, and the request closes only
when a Bitcoin payment of at least the amount, to the requested script, is
proven the same way deposits are. Signing that payment needs whoever holds
the lock key. It belongs in threshold custody (`custody-mpc`), which can
still refuse or steal. An unpaid release stays open and visible.

The bridge's books always satisfy `supply + owed = locked`: every wrapped
satoshi is backed by one proven locked and not yet proven released.

## Not built

- An Ethereum asset route: the light client proves finality, but no
  receipt or log proof is checked yet.
- A zero-knowledge light client: proving the sync committee's BLS
  signatures inside a STARK is a research project, not an increment.
- Wiring the bridge into the node's state; it is a tested library.
