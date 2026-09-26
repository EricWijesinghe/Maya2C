# 21 — Weakness map, prior art and beat bars

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 21 (no product code)

> DONE WHEN: WEAKNESS_MAP.md and BEAT_BARS.md exist with dated sources;
> prior-art files exist for every planned headline feature; the competitive
> harness runs against at least one other chain; ADR-launch-scope is updated;
> reports/21-strategy.md is complete.

| Condition | Result |
|---|---|
| `docs/strategy/WEAKNESS_MAP.md`, dated sources | **yes**: 11 weaknesses, researched by web search on 2026-09-26, each linked. Ethereum L2s, Aptos, Cosmos, Polkadot, Avalanche and TON **not researched**, and the page says so |
| `docs/strategy/BEAT_BARS.md` | **yes**: 9 bars. None is beaten, 1 ties, 1 is worse |
| Prior-art files for every headline feature of 22–30 | **yes**, 10 files. 7 say "not searched" or "partially searched", and such a file authorizes no superlative |
| Competitive harness runs against another chain | **no.** `benches/competitive/run.sh` runs and records, but no other chain's devnet binary is installed here, so each chain prints SKIPPED. It runs the primitive-level comparison only |
| ADR-launch-scope updated | **already recorded** in ADR-016 § "Adoption-critical additions". One inaccuracy found (below) |
| New standing order in CLAUDE.md | added in this branch's CLAUDE.md update |

## What the research changed

- **Maya2C is not the first post-quantum chain.** QRL has run hash-based
  signatures since its 2018 genesis and is adding ML-DSA. Every "first PQ L1"
  phrasing is now forbidden by `docs/prior-art/pq-from-genesis.md`. The
  accurate claim is narrower: a stateless *hybrid* on every transaction from
  genesis.
- **Post-quantum makes node cost worse.** The weakness map lists node cost as
  a giant's weakness, and the honest beat bar shows Maya2C behind on it,
  because a transfer is 13 KB. This is the strongest argument in the tree
  for signature pruning and key hashes on the wire (`reports/13-pq-weight.md`).
- **Resource types did not stop the largest 2025 Sui exploit.** Cetus lost
  ~$223M to an arithmetic overflow in a shared math library. Master Prompt
  23's resource semantics alone would not have blocked it; its invariants and
  rate limits might. The exploit replay table has to test that, not assume it.

## Competitive harness output

```
$ ./benches/competitive/run.sh
SKIPPED  bitcoind devnet: binary not installed
SKIPPED  geth devnet: binary not installed
SKIPPED  solana-test-validator devnet: binary not installed
SKIPPED  sui devnet: binary not installed

== primitive level (same machine, same run; not a chain comparison) ==
Ed25519 (classical)         18680 verify/s   pk    32 B   sig     64 B
ML-DSA-65                    5620 verify/s   pk  1952 B   sig   3309 B
Hybrid 65 + 128s             1083 verify/s   pk  1984 B   sig  11165 B
```

On this machine, a Maya2C signature costs about 17x more to verify than the
Ed25519 signatures Solana, Sui and Aptos use. That is the price of the
post-quantum guarantee, stated rather than hidden.

## Found

ADR-016 says smart-account validation and "contract resource/capability
checks" are launch core, and that both are "libraries today (`smart-account`,
`contract-safety`)". **`contract-safety` does not exist** in this tree.
Master Prompt 23 builds it; until then the ADR's sentence overstated the
tree.
