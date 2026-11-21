# Block time

Three block-time conventions exist in the tree: 1 s, 2 s and 15 s. This
page says which one the running consensus produces, where each of the others
comes from, and what each wrong assumption would break. It is a fact page;
the decisions it leaves open are listed at the end for Eric.

**Canonical figure: about 1 s per block** (measured 0.96–1.03 blocks/s idle;
~1.04 s/block over a 24 h run). Every "days", "months" and "year" count in
code or an ADR converts at 86,400 blocks a day unless it says otherwise.

## 1. What the consensus produces

The chain is DAG-BFT (ADR-027). A block is derived from each committed
anchor, never proposed (`crates/node/src/consensus/bft/mod.rs:11`), anchors are
even rounds (`crates/node/src/consensus/bft/driver.rs:529`), and a validator
proposes at most once per `round_interval_ms` unless a full batch waits
(`crates/node/src/genesis.rs:227-231`). So the floor is
`2 × round_interval_ms` per block.

| Setting | Value | Where |
|---|---|---|
| `bft.round_interval_ms` default | 500 | `crates/node/src/genesis.rs:420-422` |
| `bft.anchor_timeout_ms` default | 1,000 | `crates/node/src/genesis.rs:412-414` |
| testnet genesis | sets neither, so both defaults apply | `docs/site/public/testnet/genesis.json:24-33` |
| frozen binary bb4e0940 | same two defaults (`git show bb4e0940:crates/node/src/genesis.rs`) | checked 2026-11-12 |
| PoW retarget on a DAG-BFT chain | off: `fixed_target: true` (ADR-035) | `crates/node/src/consensus/chain.rs:141-146`, selected at `genesis.rs:673` |

The hash in the genesis commitment covers `round_interval_ms`
(`crates/node/src/genesis.rs:1225`), so every node on a chain shares it. No
mainnet genesis in the tree sets it; a mainnet that leaves it out gets 500 ms.

### Measured

| Run | Figure | Source |
|---|---|---|
| devnet, 4 validators, idle | **0.96–1.03 blocks/s** at `round_interval_ms = 500` | `reports/18-economics.md:103` (the `bft_devnet.py` script it cites is no longer in the tree) |
| `cargo xtask bft-devnet` | ~0.95 blocks/s | `docs/site/authored/quickstart.mdx:255` |
| 24 h proving run at 6fcfc65e (the freeze lineage, same constants as bb4e0940) | ~3,470 blocks/h in each undisturbed hour = **0.964 blocks/s, 1.04 s/block**; 73,178 blocks in 24 h overall (0.85/s, includes a 1.2 h laptop sleep and ~4 h of host contention at ~1,300–3,200/h) | `reports/proving/2026-11-09.md:9-40` |
| under load, 400 transfers a block | ~2.6 s/block; one run degraded to 28 s/block | `reports/bench/2026-10-25-tps-series.md:17,31` |
| live testnet (`status.maya2c.dev/api/status`, explorer `average_block_time`) | NOT VERIFIED on 2026-11-12: both endpoints answered Cloudflare error 1033 (tunnel down) | `docs/site/src/components/NetworkStatus.astro:68` reads it |

Conclusion: the rate is set by genesis, not emergent, and the measured
figure matches the floor. One epoch (`epoch_blocks = 3,600`) is about one
hour; `halving_epochs = 35,064` is about four years (DECISIONS 2026-10-27).

## 2. The other two conventions, site by site

### 15 s — the PoW difficulty target

`TARGET_BLOCK_TIME = 15` (`crates/node/src/consensus/difficulty.rs:32`) is the
retarget spacing for a proof-of-work chain. On a DAG-BFT chain the retarget is
bypassed (`fixed_target`), so the constant steers nothing; it survives in
comments and in a few derived values:

| Site | Assumes | At the measured ~1 s it is | Effect |
|---|---|---|---|
| `crates/governance/src/limits.rs:29-33` `MIN_VOTING_BLOCKS = 480` "about two hours" | 15 s | 8 minutes | a vote can open and close in 8 min, the thing the comment says it prevents. **Consensus constant.** |
| `crates/governance/src/limits.rs:50-51` `MIN_TIMELOCK_BLOCKS = 960` "about four hours" | 15 s | 16 minutes | the exit window before a passed proposal binds. **Consensus constant.** |
| `crates/node/src/governance/work.rs:46-52` `WORK_HALF_LIFE_BLOCKS = 5,760` "about a day" | 15 s | 1.6 hours | miner weight decays 15× faster than the comment intends; a week of absence is reached in 11 hours. **Consensus constant.** |
| `crates/crypto-pq/src/agility/mod.rs:28-29` `MIN_MIGRATION_WINDOW = 172,800` "about 30 days at 15 s" | 15 s | 2 days | ADR-082 §H3 and DECISIONS 2026-11-08 already treat it as 2 days (council lead 13.6 d). The 30-day intent is lost. **Consensus constant.** |
| `crates/node/src/crypto/dag/mod.rs:80-81` `EPOCH_LENGTH = 30,000` "5.21 days" | 15 s | 8.3 hours | every miner regenerates the 4 GiB dataset three times a day; `DAG_ACTIVATION_HEIGHT = EPOCH_LENGTH` is 8.3 h after genesis, not five days. **Consensus constant** for the mining lane. |
| `crates/node/src/network/pq/rotation.rs:42-56` `ROTATION_INTERVAL_BLOCKS = 1,000` "a little over four hours"; `ROTATION_FALLBACK = 1,000 × 15 s` | 15 s | 17 minutes by height; the wall-clock fallback stays 4.2 h | re-handshakes 15× more often than designed; transport only, not consensus |
| `bins/maya2c-node/src/rejoin.rs:111-131` | 15 s as a **cap** on the observed interval | observed ~1 s wins | correct by construction; the 15 s default applies only with no history |
| `spec/05-consensus.md:16` CON-5 "15 s target spacing" | 15 s | n/a | a PoW-chain rule; true for chains with retargeting, inert on DAG-BFT. Spec should say so. |
| `docs/site/authored/start-mining.mdx:188-189` "Target block time 15 seconds" | 15 s | wrong on the live chain | fixed in this PR |

### 2 s — the smart-account policy day

| Site | Assumes | At ~1 s | Effect |
|---|---|---|---|
| `crates/smart-account/src/policy.rs:6-7` `BLOCKS_PER_DAY = 43,200` | 2 s | 12 hours | `daily_limit` and `per_recipient_daily` reset twice a real day: a 2× overspend against the policy's intent. Pinned as open F7 by `crates/smart-account/tests/account_tests.rs:989-994`; ADR-056, ADR-086 §14.1. Not linked into the node today. |
| `econ/src/params.rs:48-49` `block_secs: 2`, `epoch_blocks: 43_200` | 2 s | model only | SIM crate outside the workspace; its "day" is twice the chain's epoch. |
| `docs/adr/ADR-018-async-execution.md:44` "~4 s at 2 s blocks" | 2 s | ~2 s | design note; no code. |

### 1 s — the governance and economics convention (matches the measurement)

Invariant 34 (`docs/invariants.md:257`), ADR-082 (`:49,53,86-95`),
`MIN_UPGRADE_LEAD_BLOCKS = 2,592,000` and `MAX_SCHEDULE_HORIZON_BLOCKS =
31,536,000` (`crates/governance/src/limits.rs:84-95`),
`COUNCIL_SUNSET_LEAD_BLOCKS` "13.6 days" (`crates/node/src/state/upgrades.rs:32-40`,
DECISIONS 2026-11-08), `halving_epochs = 35,064` (DECISIONS 2026-10-27), the
ADR-068 tail rate (`:49-55`), ADR-135's `QUARTER_BLOCKS = 7,776,000` and
`YEAR_BLOCKS = 31,536,000`, and the comment at
`crates/node/src/consensus/chain.rs:136-139` ("on one-second blocks").

## 3. What a wrong rate breaks

- **Activation heights.** "v3 + 3 months" is 7,776,000 blocks at 1 s. Computed
  at 15 s it is 518,400: v4 would go live about six days after v3, 15× early.
  Computed at 2 s it is 3,888,000: six weeks, 2× early. Every v4 lifecycle
  count in ADR-135 (`YEAR_BLOCKS`, `GRACE_BLOCKS`, `MAX_COMMIT_AGE = 86,400`,
  `MIN_INTERVAL = 3,600`, the paymaster daily window) converts the same way.
- **Compiled leads.** `MIN_UPGRADE_LEAD_BLOCKS` is 30 days at 1 s and 450 days
  at 15 s; a lead sized at 15 s would make a governance upgrade unschedulable
  inside the one-year horizon.
- **Rate-sized constants sized at 15 s** (table above) are 15× shorter in
  wall-clock than their comments claim. The governance floors are the ones
  that matter: 8-minute votes and 16-minute timelocks.
- **Economics.** The ADR-068 tail is 1.07 %/yr at 1 s and 0.07 %/yr at 15 s;
  the halving era is 4 years at 1 s and 60 at 15 s. The accepted figures
  (DECISIONS 2026-11-07) are the 1 s ones.
- **Operations.** `max_pause_blocks` (6 hours) is computed at the ceremony
  from the mainnet rate (ADR-050); at 1 s it is 21,600.

## 4. Open for Eric (consensus constants; not changed by this PR)

1. **Pin the convention.** Make 1 block/s the written rule: add a line to
   invariant 34's text, and have `xtask genesis-check` refuse a mainnet
   genesis whose `round_interval_ms` is not 500 (today it only bounds
   `anchor_timeout_ms`). Without the check the rate is a default, not a fact.
2. `MIN_VOTING_BLOCKS` 480 → 7,200 and `MIN_TIMELOCK_BLOCKS` 960 → 14,400 to
   keep the "two hours / four hours" the comments promise, or accept 8/16 min.
3. `WORK_HALF_LIFE_BLOCKS` 5,760 → 86,400 for a one-day half-life.
4. `MIN_MIGRATION_WINDOW` 172,800 → 2,592,000 for the intended 30 days. That
   moves `COUNCIL_SUNSET_LEAD_BLOCKS` from 1,172,800 (13.6 d) to 3,592,000
   (41.6 d); ADR-082 H3 and DECISIONS 2026-11-08 would need the new figure.
5. DAG `EPOCH_LENGTH` 30,000 → 450,000 for a 5.2-day dataset epoch (and the
   activation height with it), or accept an 8.3 h regeneration for miners.
6. `BLOCKS_PER_DAY` 43,200 → 86,400 in `smart-account` (ADR-086 F7) before
   any "days" copy ships.
7. `TARGET_BLOCK_TIME` itself: rename to `POW_TARGET_BLOCK_TIME` or document
   it as PoW-only, so no new comment derives a wall-clock figure from it.
8. `ROTATION_INTERVAL_BLOCKS` and `ROTATION_FALLBACK` (transport, not
   consensus): 15,000 → 1,000 s for the fallback, or 1,000 → 15,000 blocks.

All eight change frozen code (`crates/node`, `crates/governance`,
`crates/crypto-pq`) and the first six change consensus; they belong to the
post-freeze upgrade path (ADR-082), not to a doc PR.
