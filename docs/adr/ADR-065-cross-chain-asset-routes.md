# ADR-065: Which cross-chain asset routes reach consensus, and in what order

Status: Proposed (2026-10-27). Needs Eric's decision: wiring a bridge into the
node's state changes what a validator can be made to believe, which is a
security decision (CLAUDE.md, "When to stop and ask Eric").

## Context
[docs/crosschain.md](../crosschain.md) records three routes and that **none is
called by consensus**:

| Route | State today |
|---|---|
| Hash-locked swap (`htlc-watcher`, node HTLC records) | node records and a watcher exist; needs each chain's own finality |
| Bitcoin lock/mint (`btc-bridge` over `btc-spv`) | tested library; deposits proven by most-work header chain to a stated depth; releases need a lock key, so custody |
| Ethereum light client (`interop::beacon`) | proves Ethereum finality; no receipt or log proof, so no asset route |

The blind spot the strategy work names is bridge trust
([BEAT_BARS](../strategy/BEAT_BARS.md): Ronin lost funds to 5 of 9 keys). A route
that holds funds behind a few keys repeats that weakness. A route that needs no
party to stay honest is the only one that improves on it.

## Options
1. **Swaps only.** Hash-locked swaps between Maya2C and chains with hash-lock
   scripts. No custody, no new consensus rule. Trusts each chain's finality and a
   party staying online until its timeout. Moves no value into a smart contract
   on the other chain.
2. **Bitcoin inbound, disclosed custody outbound.** Wire `btc-bridge` deposit
   verification into the state transition (mint wrapped BTC for a deposit
   proven to `min_confirmations`), and release BTC through threshold custody
   (`custody-mpc`). Inbound is trust-minimized, outbound is a key set that can
   refuse or steal. The invariant `supply + owed = locked` is already tested.
3. **Ethereum receipt proofs.** Extend the beacon light client with receipt/log
   proofs so a validator can verify an Ethereum deposit event. Largest new
   verification surface; the zero-knowledge version is a research project.

## Recommendation
Option 1 first (nothing new reaches consensus, and it works today up to
operations), then option 2 behind an **activation height of `u64::MAX`** until
Eric writes one down (CLAUDE.md order 3), with these conditions:

- the custody key set is stated publicly with its size and threshold, and the
  release path says "a key set that can refuse or steal" in every user-facing
  string (order 10);
- a per-epoch mint cap, so a proof bug is bounded rather than total;
- `min_confirmations` chosen against the reversal table in `docs/crosschain.md`
  and the value at risk, written in the genesis;
- the bridge's books asserted in the invariant guard, not only in its tests.

Option 3 waits until options 1 and 2 have run on the public network.

## Not decided here
Which assets, the cap values, the custody threshold and who holds the keys. None
of those is a code decision and none has a measured basis yet.

## Consequences
- Option 2 adds a state prefix (invariant 25: a new prefix needs a numbered
  invariant once a test pins it), conformance vectors and a re-genesis.
- Until it ships, the BEAT_BARS "bridge trust" row stays "no bridge".
