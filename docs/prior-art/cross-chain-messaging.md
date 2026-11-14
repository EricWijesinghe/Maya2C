# Prior art: Generic cross-chain messaging over verified headers

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "IBC ICS-004 channel packet ordering light client Avalanche ICM Warp messaging Chainlink CCIP cross-chain messaging"; nothing on ICM or CCIP surfaced in it, so those rows are from the parity survey and marked thin).

Feature: parity row P-34, [ADR-122](../adr/ADR-122-cross-chain-messaging.md). Supersedes the "not searched" entry in `trustless-interop.md` for messaging.

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| IBC ICS-004 | Channels with ordering, exactly-once delivery and module permissioning; packets authenticated and ordered by on-chain light clients; one channel per connection per client. IBC v2 lets packets reference source/destination clients directly with any verification model. | [ICS-004 spec](https://docs.cosmos.network/ibc/next/spec/core/ics-004-channel-and-packet-semantics), [GitHub spec](https://github.com/cosmos/ibc/blob/52a9094a5bc8c5275e25c19d0b2d9e6fd80ba31c/spec/core/ics-004-channel-and-packet-semantics/README.md), [IBC overview](https://ibc.cosmos.network/v4/ibc/overview) |
| IBC performance analysis (academic) | Measurements of the relayer/light-client path. | [arXiv 2303.10844](https://arxiv.org/pdf/2303.10844) |
| Avalanche ICM / Warp (thin) | Validator-set BLS attestations between subnets. | parity matrix row (not re-searched here) |
| Chainlink CCIP 2.0 (thin) | Oracle-network attestations with risk-management network. | [Chainlink press release](https://chain.link/press-releases/chainlink-launches-ccip-2-0-to-maximize-the-utility-and-reach-of-institutional-digital-assets-across-blockchains) |
| LayerZero vs IBC (vendor comparison, secondary) | External verifier sets trade light-client rigor for faster integration. | [chainscorelabs](https://www.chainscorelabs.com/comparisons/security-audits-vs-formal-verification/cross-chain-bridge-validation/ibc-inter-blockchain-communication-vs-layerzero-interoperability-architecture) |

## What Maya2C has (checked in repo, 2026-11-12)

- An Ethereum beacon light client verifying sync-committee finality and the execution payload branch (`crates/interop/src/beacon.rs:7-19`), with no committee handover and no receipt proofs (`beacon.rs:21-23`, ADR-065); execution header hashing (`eth.rs:129`); route caps (`routes.rs`). RESEARCH, nothing in consensus calls it.
- A Bitcoin SPV light client (`crates/light-client`, verified) and a `btc-bridge` library behind ADR-065's `u64::MAX`.

## How the Maya2C version differs

- **One direction, honestly**: inbound Ethereum → Maya2C over the light client and MPT receipt proofs; outbound is a committed record with a layer proof and a stated missing verifier (no EVM ML-DSA verification today).
- IBC's ordered channel without the handshake: `(src chain, src contract, nonce)` streams, exactly-once by nonce, delivered as timer-style calls with `caller() = None`.
- Count caps per route with incident reset, from the existing `routes.rs` curve.
- The BLS12-381 exposure is inherited and disclosed in every user string.

**Verdict: similar exists (IBC light clients; ICM/CCIP attestations). Differentiate by light-client-only inbound and the explicit outbound gap. No "first" claim.**
