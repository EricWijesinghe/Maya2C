# Prior art: Liquid staking receipt token

**Search status:** searched 2026-11-12 (web search; sources cited below). Cosmos LSM returned nothing in one query; **not searched**.

Feature: parity row P-6, [ADR-108](../adr/ADR-108-liquid-staking-receipt.md).

**Queries:** "Lido stETH rebasing slashing socialized"; "Jito JitoSOL stake pool slashing; Cosmos liquid staking module LSM".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Lido stETH (Ethereum) | A rebasing token whose balance tracks pooled ETH; slashing losses are socialized across all holders; a DAO-held slashing fund exists (size disputed between sources); negative-rebase protection added. Off-chain node operators chosen by the DAO. | [Lido integration guide](https://docs.lido.fi/guides/lido-tokens-integration-guide/), [Lido research: fate after slashing](https://research.lido.fi/t/fate-of-steth-and-unbonded-eth-after-slashing/8264), [known risks](https://lido.fi/how-lido-works/known-risks-and-mitigations), [negative rebase protection](https://blog.lido.fi/negative-rebase-protection-now-live/) |
| Jito JitoSOL (Solana) | A stake-pool program (Solana Labs' audited stake pool) with an exchange rate rather than rebasing; validator selection by StakeNet; slashing is not implemented on Solana, so the risk is described as theoretical. | [JitoSOL glossary](https://go.jito.network/docs/jitosol/resources/jitosol-glossary/), [technical FAQs](https://go.jito.network/docs/jitosol/faqs/technical-faqs/), [delegation criteria](https://go.jito.network/docs/jitosol/jitosol-liquid-staking/stake-pool-operations/delegation-criteria) |
| Cosmos liquid staking module | Native tokenized delegations with a global liquid-staking cap. **Not searched.** | — |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/staking`: `ValidatorRecord`, delegations keyed `(delegator, validator)`, `Unbonding { owner, validator, amount, release_epoch }` (`types.rs:33-66`), `slash` cuts self bond, every delegation and every unbonding entry by `bps` (`ops.rs:248-290`), double-sign tombstones for good (`ops.rs:229-245`). `unbonding_epochs` 7 on devnet (`params.rs:43`).
- Node-side effects: `Effect::Debit/Credit/Burn` applied in `crates/node/src/state/staking_exec.rs:129-140`; tips become the epoch reward pool (`fees.rs:14-17`).
- Multi-asset records with `U256` supply (`crates/node/src/state/asset.rs`), which the receipt can be minted in.
- No liquid staking code; `rg -i "liquid stak"` over `crates`: none.

## How the Maya2C version differs

- **Protocol-native pool, exchange-rate token** (Jito's shape), minted and burned only by the staking module; no DAO operator set: the pool delegates across the active committee by a deterministic rule.
- **Slashing is socialized by construction** because `Staking::slash` already cuts the pool's delegation like any other; the receipt rate falls. No insurance fund is promised.
- Rate arithmetic stays in 256 bits with a written bound; no 512-bit intermediate.

**Verdict: similar exists (Lido, Jito). Differentiate by protocol-native minting and deterministic delegation. No "first" claim.**
