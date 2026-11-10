# Prior art: Personal agent (an AI aide that spends within hard limits and asks before large acts)

**Search status:** searched 2026-11-10 (web search; sources cited below).

Feature: CATALOG §I row `personal-agent` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R0 idea. Builds on `agent-wallets` and `mcp-server` in the catalog.

**Queries:** "Coinbase AgentKit AI agent wallet spending limits session keys"; "ERC-4337 session keys spending limits smart account ERC-7579 modules"; "Forta Hypernative onchain monitoring AI automated response pause contract 2026".

## Who else does something similar

| Project or standard | What it does | Source |
|---|---|---|
| Coinbase Agentic Wallets | Product for AI agents: session caps, per-transaction limits, keys held in a trusted execution environment and not exposed to the model. Launch reported as 11 Feb 2026. Official page is the primary source. | [Coinbase](https://www.coinbase.com/developer-platform/discover/launches/agentic-wallets), [CDP docs](https://docs.cdp.coinbase.com/agentic-wallet/cli/welcome.md) |
| Coinbase AgentKit "Spend Permissions" | Delegated authorisations with spending limits and time restrictions. Secondary source only. | [skills.sh](https://skills.sh/coinbase/com/default) |
| ERC-4337 + ERC-7579 modules | Session keys and spending limits as account modules; a spending-limit policy typically runs as a pre/post hook that can revert. ERC-7715 (wallet permissions) and ERC-8043 (draft) are adjacent. Mostly vendor explainers; check the EIPs. | [Eco ERC-7579 guide](https://eco.com/support/en/articles/11890018-erc-7579-the-complete-guide-to-modular-smart-accounts), [ERC-8043 draft](https://ethereum-magicians.org/t/erc-8043-atomic-erc-7579-module-install-and-execute/25901) |
| Hypernative | Monitoring that can act: automated pause or position unwind, with a role granted to the service. Self-reported results. | [Hypernative](https://hypernative.io/products/onchain-monitoring) |

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/smart-account/src/policy.rs`: `Policy` with `daily_limit`, `per_recipient_daily`, and `delay_above` / `delay_blocks` (large transfers wait and can be cancelled). `SessionScope` has `recipients`, `max_amount` per transfer, and `expires_at`; `SessionScope::allows` checks all three.
- `bins/maya2c-mcp`: the MCP server's tools are `decode_transaction`, `review_intent`, `explain_error`, `rpc_call`. None signs. (Checked in the tool list, 2026-11-10.)
- Limits are state of the smart account, checked by validators on every operation, not by an off-chain service.

## How a Maya2C version would differ

- **Limits enforced by consensus state.** A session cannot exceed its scope on any node, because validators check the account's own policy. Coinbase's limits are enforced by its wallet service and TEE. ERC-7579 hooks are enforced by the account contract on Ethereum, which is closer; the difference is that Maya2C's limits are part of the chain's state rules, not contract code a user installs.
- **No signing path in the agent's server.** The MCP server reads, decodes and reviews. A personal agent that holds only a `SessionScope` could not move funds outside it. This is a property of the current tool list, not a tested guarantee of an agent product, which is not built.
- **Delay with cancel.** Large transfers wait `delay_blocks` and can be cancelled, which is the "asks before large acts" step.

Not built: the agent itself, a session-key issuance flow, and any kill switch. The catalog's "kill switch" is not in the repo and is not claimed.

**Verdict: similar exists (Coinbase Agentic Wallets, ERC-4337/7579 session modules). Differentiate by consensus-enforced session scope and a signing-free agent server. Not a novelty claim.**

**forbidden**: "first AI wallet", "only agent with spending limits", "the agent can never move funds".
