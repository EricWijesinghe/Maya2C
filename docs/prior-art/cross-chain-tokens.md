# Prior art: Cross-chain token standards with per-bridge limits

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "xERC20 ERC-7281 bridge rate limits Chainlink CCT cross-chain token standard LayerZero OFT").

Feature: parity row P-23, [ADR-120](../adr/ADR-120-cross-chain-token-standard.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| ERC-7281 (xERC20) | The issuer allowlists bridges and sets per-bridge mint and burn rate limits; a compromised bridge is bounded by its allocation; one canonical token across bridges. | [Ethereum Magicians thread](https://ethereum-magicians.org/t/erc-7281-sovereign-bridged-tokens/14979), [xERC20 bridge spec](https://hackmd.io/@arjunbhuptani/xerc20-bridge-spec), [Connext docs](https://docs.connext.network/usecases/xerc20) |
| Chainlink CCT over CCIP | Issuer-owned token pools with per-lane token-bucket limits (capacity + refill); Lido chose it for wstETH. | [Lido blog](https://blog.lido.fi/cross-chain-security-principles-why-lidos-network-expansion-committee-chose-chainlink-ccip/) |
| LayerZero OFT | Token tied to LayerZero's verifier configuration; a rate-limiter module with linear-decay buckets per destination. | [LayerZero docs](https://docs.layerzero.network/v2/developers/evm/stablecoin-oft/extensions) |
| Survey (primary, academic) | A comparison of the three standards. | [arXiv 2603.06388](https://arxiv.org/pdf/2603.06388) |

## What Maya2C has (checked in repo, 2026-11-12)

- Assets with fixed supply and no mint (`crates/node/src/state/asset.rs:17-25`); ADR-103 adds one authority and a cap.
- `crates/interop::routes` value caps growing with clean days (`routes.rs:3-8`), a library nothing in consensus calls; ADR-065's bridge plan with activation `u64::MAX`.
- No cross-chain token, no bridge in consensus.

## How the Maya2C version differs

- The **route is the mint authority**, and it mints only against a proven inbound event (ADR-122 message or ADR-065 deposit), never on a relayer's word.
- Route caps use the existing incident-reset growth curve rather than a token bucket; a capped delivery **waits** (nonce does not advance) instead of failing.
- One canonical asset per foreign token by a first-registration mapping, as ERC-7281 intends.
- Outbound burns are recorded under the state root; nothing on the destination verifies them yet, stated plainly.

**Verdict: similar exists (xERC20, CCT, OFT). Differentiate by proof-only minting and waiting deliveries. No "first" claim.**
