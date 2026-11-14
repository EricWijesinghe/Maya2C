# Prior art: aid tracing (disaster-relief funds traced to recipients, with allow-lists and limits)

**Search status:** searched 2026-11-10 (web search; snippets only, primary pages not fetched; weights stated per row).

Feature: CATALOG §I row `aid-tracing` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R1 idea.

**Queries:** "UNHCR Building Blocks blockchain WFP cash assistance refugees Ethereum" (shared with [emergency-identity.md](emergency-identity.md)).

## Who else does something similar

| Project or paper | What it does | Source |
|---|---|---|
| WFP and UNHCR Building Blocks | Cash-transfer system on a permissioned Ethereum network (Parity, Proof-of-Authority) in Jordan camps, with a central authority controlling participants. 2019 reporting: about 106,000 Syrian refugees redeemed transfers in Azraq and Zaatari. A later source claims over 400,000 refugees assisted; this figure's primary source was not retained. Critics argued the narrow permissioned scope could be met by a database. (P/S, snippets) | [ITU, WFP and blockchain (2020)](https://www.itu.int/hub/2020/04/how-the-world-food-programme-uses-blockchain-to-better-serve-refugees/); [Parity case note](https://www.parity.io/blog/parity-ethereum-to-aid-100-000-refugees) |

Building Blocks is the deployed precedent. The permissioned design and its critics are the context a Maya2C version must answer.

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/smart-account/src/policy.rs`: `Policy.allow_list` ("only these recipients may be paid"), `per_recipient_daily` ("most one recipient may receive per day"), `daily_limit`. The header says these are "evaluated by the chain, not by app code".
- `SessionScope { recipients, max_amount, expires_at }` in the same file: an agent or app key limited to listed recipients, a per-transfer cap and an expiry height.
- `crates/rwa`: `rg -n "allow|whitelist|kyc" crates/rwa/src` returns no allow-list or KYC gate; hits are clippy attributes and an error doc. The RWA crate has no recipient gate of its own.
- The consensus path that calls `Policy` was **not traced** in this check; confirm before any claim that the limit is enforced at block validation.

## How a Maya2C version would differ

- **Limits in consensus.** Per-recipient daily caps and allow-lists enforced by the chain, so an aid operator's app cannot overpay. Building Blocks relied on a permissioned network's central authority instead. Design claim, not built.
- **Privacy is the hard part.** Recipient allow-lists on a public chain reveal who receives aid. Building Blocks used a permissioned chain for this reason, and its critics noted the cost. A Maya2C version would need the viewing-key work in [privacy-viewing-keys.md](privacy-viewing-keys.md) (not searched) before any recipient list goes on chain.
- **Cost of the trade.** Consensus-enforced limits remove an operator's discretion in a crisis, which is sometimes the point and sometimes the risk.

**Legal and ethical gates.** NGO and legal partners, named in the CATALOG gate. Recipient lists are personal data under GDPR and under UNHCR's own policy (see [emergency-identity.md](emergency-identity.md)); pseudonymous addresses tied to people may still be personal data, a counsel question. No recipient data on a public chain without an opinion. Humanitarian donors may impose their own data rules.

**Verdict: similar exists and is deployed (Building Blocks, permissioned Ethereum). Maya2C differs at the design level: chain-enforced recipient limits and allow-lists in the smart-account policy. Not built; the privacy design and a partner's legal review come before any aid deployment.**

**forbidden**: "first aid-tracing chain", "only blockchain aid tracking", "unprecedented relief-fund transparency", "tracks every aid recipient on Maya2C".
