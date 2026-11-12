# Prior art: Light-lag ledger (payments between distant sites, settled when links return)

**Search status:** searched 2026-11-10 (web search and RFC page fetch; sources below). No interplanetary-ledger work beyond one arXiv paper was found, so absence of other work here is a finding about this search, not a proof.

Feature: CATALOG §I row `light-lag-ledger`, built from `crates/l2-flash` and a planned delay-tolerant link.

## Who else does something similar

| Project or work | What it does | Source |
|---|---|---|
| Blockstream Satellite | Broadcasts Bitcoin blocks and transactions from ground teleports to geostationary satellites (DVB-S2 since version 2.0); receive-only, so users need another channel to send transactions | [Blockstream](https://blockstream.com/satellite/), [Satellite 2.0 post](https://blog.blockstream.com/en-announcing-blockstream-satellite-2/) |
| Bundle Protocol v7, RFC 9171 | Store-and-forward delay-tolerant networking; Proposed Standard (updated by RFC 9713 and RFC 9758) | [RFC 9171](https://www.rfc-editor.org/rfc/rfc9171) |
| SpaceChain | Blockchain nodes on satellites since 2018; a 2-of-3 multisig wallet where one signature can be computed on the satellite | [Geospatial World](https://www.geospatialworld.net/news/spacechain-successfully-launches-first-blockchain-node-low-earth-orbit/), [Satellite Today](https://satellitetoday.com/technology/2022/07/12/spacechain-completes-new-blockchain-testing-milestone) |
| "Bitcoin as an Interplanetary Monetary Standard with Proof-of-Transit Timestamping" | arXiv preprint; argues synchronous cross-planet mining is infeasible, proposes asynchronous settlement over DTN, adds a Lightning timelock margin of about ceil((RTT+J)/block interval) blocks | [arXiv 2508.20591](https://arxiv.org/abs/2508.20591v1) |

Not found: an IETF interplanetary-payments draft, or an interplanetary ledger with its own consensus protocol. The IETF search was not done beyond RFC 9171.

## How a Maya2C version would differ (each backed by the repo)

- **Time is block height.** Invariant 9 (`docs/invariants.md`) bans timestamps for freshness. The HTLC expiry is `expiry_height: u64` (`crates/l2-flash/src/htlc.rs:69`), with a per-hop margin `HOP_EXPIRY_DELTA = 24` blocks (`htlc.rs:29`). A light-lag margin would have to be chosen; no such number exists in the repo.
- **Signatures are large.** A hybrid signature is 11,165 bytes and incompressible (`hal/radio-transport/src/lib.rs:5-8`). A store-and-forward link must budget for that — a cost, not an advantage.
- **Nothing is built.** `features.toml` lists "CCSDS delay-tolerant networking (BPv7)" as RESEARCH / planned with no paths and no tests; `docs/architecture-vision.md` tags it PLANNED. `satellite-uplink` and `leo-free-space-laser-mesh` are also planned. `hal/link-sim` models links as SIM (`hal/link-sim/src/lib.rs:1-15`).
- The difference is a design intention (post-quantum channels settled by height). Nothing runs over a delayed link today.

## Legal and ethical gates

- Spectrum and orbital licensing for any real space segment (national regulator, ITU filings). Not researched here.
- Export-control review for space hardware and cryptography. Not researched here.
- Civil use only. No space hardware is in scope until the DTN subsystem leaves "planned".

## What may be said publicly

- Accurate: "Maya2C's channel timelocks count blocks, not seconds."
- **forbidden**: "first interplanetary payment network", "only chain built for space", "unprecedented", "space-ready", "interplanetary ledger".

**Verdict:** RESEARCH, no promotion case. Promotion would need a BPv7 transport with tests, a stated channel margin for a named delay, and a sourced licensing path.

