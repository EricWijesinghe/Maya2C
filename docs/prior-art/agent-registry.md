# Prior art: Agent registries and agent-to-agent payments

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "AI agent registry on-chain ERC-8004 agent-to-agent payments x402 AP2 session keys spending limits"). Sources are mostly blogs and vendor pages; spec status not checked against 8004.org directly.

Feature: parity row P-48, [ADR-130](../adr/ADR-130-agent-registry-and-agent-payments.md); related [personal-agent.md](personal-agent.md) (searched 2026-11-10).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| ERC-8004 "Trustless Agents" | Identity registry (an ERC-721 pointing at an agent card), reputation registry, validation registry; sources disagree on whether validation is deployed. | [Ethereum Magicians](https://ethereum-magicians.org/t/erc-8004-trustless-agents/25098?page=7), [eco.com](https://eco.com/support/en/articles/14730445-erc-8004-trustless-agent-identity), [stablecoininsider](https://stablecoininsider.org/erc-8004/) |
| Session keys on ERC-4337 wallets | Agent gets a session key with spend caps and allow-listed counterparties, bound to the principal through ERC-8004 so provenance is discoverable. | [eco.com](https://eco.com/support/en/articles/14730445-erc-8004-trustless-agent-identity), [Nevermined](https://nevermined.ai/blog/building-agentic-payments-with-nevermined-x402-a2a-and-ap2) |
| AP2 (Google) | Signed mandates proving a human authorised a kind of spending. | [yewjin substack](https://yewjin.substack.com/p/agent-payments-protocol-ap2-and-the), [PIAIC crash course](https://agentfactory.piaic.org/docs/payment-enabled-agents-crash-course) |
| x402 / A2A x402 extension | HTTP-402 settlement, stablecoin payments between agents. | [Splunk blog](https://www.splunk.com/en_us/blog/artificial-intelligence/the-agent-economy-when-ais-pay-each-other.html), [chainup](https://www.chainup.com/?p=13338) |
| 0xGasless | Agent SDK with contract-enforced caps. | [whitepaper](https://ik.imagekit.io/mkkdp9u7r/0xgasless/0xGasless_Trustless_AI_Agent_Infrastructure_Whitepaper.pdf) |
| Analyses | Adoption marginal; identity ahead of payments; budget enforcement against creative workarounds an open problem. | [stablecoininsider](https://stablecoininsider.org/erc-8004/), [moklabs](https://moklabs-research.pages.dev/reports/security/2026-03-19-ai-agent-identity-payments) |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/smart-account` `SessionScope { recipients, max_amount, expires_at }` and a daily limit (`policy.rs:12-61`); ADR-107 promotes them to chain-evaluated rules on v9 frames (session keys sign `Transfer` and nothing else).
- `bins/maya2c-mcp`: four read/draft tools, writes refused off loopback (`main.rs:1-15`; ADR-084).
- Payment requests `maya:<bech32m>?amount=&label=&message=&exp=&id=` (ADR-086 D6).
- No agent registry, no agent id, no receipt format.

## How the Maya2C version differs

- An agent **is** a session key plus a registry record (`ag:`) bound to one owner and one key; limits are enforced by the node's frame verification (ADR-107), not by SDK or contract code the agent could route around.
- Revocation is one owner transaction that also deletes the session record (kill switch); large payments take the owner-cancellable delayed path.
- Receipts and cards are off-chain documents named by hash; no reputation on chain; payments are plain transfers, no new kind.

**Verdict: similar exists (ERC-8004 registries, 4337 session keys, AP2 mandates, x402). Differentiate by node-enforced scope on native accounts and one-transaction revocation. No "first" claim.**
