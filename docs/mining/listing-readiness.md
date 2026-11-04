# Listing readiness: what mining sites need from a new coin

Research date: **2026-11-04**. Facts, no estimates. Each row cites the page it came from
and what that page said on the day it was read. Where a source did not say
something, the row says so. Nothing here is a promise that any site will list
Maya2C.

## Sources read

| # | Source | Read on | What it says that matters |
|---|---|---|---|
| S1 | MiningBoard pool spec v1, `https://miningboard.com/pools/spec/v1` | 2026-11-04 (page shows no date) | Pool-stats document: public GET, no auth, JSON. Required: `coin` (ticker `^[A-Z0-9]{2,10}$`), `pool.hashrate` (H/s, > 0), `pool.miners`. Optional: `pool.workers`, `blocks_24h`, `last_block_at`, `fee_percent`, `payout_scheme`, `min_payout`, and a `network` block (`hashrate`, `height` required inside it; `difficulty`, `block_reward`, `block_time` optional). Hashrates must be raw H/s, never TH/s or strings. Polled every 5 min; values refreshed at least hourly. Pool-global values. |
| S2 | minerstat, "How we verify a coin before listing", `https://minerstat.com/blog/how-we-verify-a-coin-before-listing` | 2026-11-04; page states **published May 12, 2026** | Listing is treated as a verification task. Needed: chain height and block times that agree across sources; difficulty and an implied hashrate (recomputed from difficulty and block times, not taken from reported figures); block reward read from real coinbase transactions; price and market depth; **at least two independent sources** from different classes (direct chain access, chain-operated explorer, third-party aggregator). Confidence score (source agreement, freshness, stability) must stay above threshold for a multi-day window; gaps restart the clock. Human review of exchanges, development, pool concentration and reorg history. Monitoring continues after listing; coins are removed if the network stalls or confidence stays low for weeks. The page does not list algorithm or explorer as required inputs, and it does not describe the submission form in detail. |
| S3 | WhatToMine, `https://whattomine.com/coins` | 2026-11-04 (page: "Last update at 2026-11-04 13:38:13 UTC") | The coin table shows: algorithm, hashrate, power, block time, block reward, last block, difficulty, NetHash. **No documented listing request process and no stated requirements.** Contact link points to `/contacts/new`, whose purpose the page does not state. Per-coin data is exposed as a JSON link; its schema is not documented on the page. |
| S4 | Awesome Miner support article, "can you add a new coin with profit information", `https://support.awesomeminer.com/support/solutions/articles/35000085824-can-you-add-a-new-coin-with-profit-information-` | 2026-11-04 (search snippet; article date not seen) | Third party. States that a coin added to WhatToMine or CoinCalculators with properties and profit information appears automatically in Awesome Miner. Confirms WhatToMine's coin data is the feed for downstream tools. Does not describe WhatToMine's own process. |

### Not verified

- **MiningPoolStats**: the search returned no document describing how to add a
  coin or a pool. The official site was not fetched for this file. Status: unknown.
- **hashrate.no**: not searched in this pass. Status: unknown.
- **minerstat's submission form** (`/submit`, linked from S2): the page was not
  read. Its requirements beyond S2 are unknown.
- **Minerstat coins API**: an older third-party Medium post (undated in the
  search snippet) says the coins endpoint requires crediting the project for
  public or commercial use. Not checked against current minerstat terms.
- **Blockchain-explorer requirement**: none of S1 to S3 lists a public block
  explorer as a requirement. S2 names chain-operated explorers as one allowed
  source class.

## Fields the sites ask for, and what this repo provides

Mapping from the fields in S1 to S3 onto the endpoints added in this change.
Values come from a running node or pool, never from a constant the site could
read as a measurement.

| Field asked for | Source in S1 to S3 | Provided by | Status |
|---|---|---|---|
| Algorithm name | S3 | `GET /api/mining-stats` → `algorithm` | Label `Maya2C DAG (BLAKE3)`, taken from `docs/dag-pow.md`. No node constant carries it. |
| Network hashrate (H/s) | S1 `network.hashrate`, S2, S3 NetHash | explorer `network_hashrate_hs` | Estimated from indexed blocks over a 30-block window. Estimate, labelled as such. |
| Difficulty | S1 `network.difficulty`, S3 | explorer `difficulty` | Expected hashes of the latest block's target. |
| Target block time | S3 block time | explorer `target_block_time_s` = 15 | From `TARGET_BLOCK_TIME` in the node. |
| Measured block time | S2 | explorer `measured_block_time_s` | Mean seconds over the sample window. |
| Height | S1, S2 | explorer `height` | Highest indexed height. |
| Last block time | S1 `last_block_at`, S3 | explorer `last_block_timestamp`; pool `last_block_at_ms` | Unix seconds (explorer) and milliseconds (pool). Different units; listing sites must convert. |
| Block reward | S1 `network.block_reward`, S2, S3 | explorer `block_reward` is always `null` | **Not provided.** The chain pays an epoch budget split across shares (ADR-044), not a per-block coinbase, so there is no single number to publish. Needs an owner decision on how to state it. |
| Pool hashrate, miners, workers | S1 `pool.*` | pool `GET /api/pool-stats` | Provided. Aggregates. |
| Pool blocks in 24 h, last block | S1 | pool `blocks_24h` (null when the scan saturates), `last_block_at_ms` | Provided. |
| Fee, payout scheme, minimum payout | S1 | pool `fee_percent`, `payout_scheme` = `PPLNS`, `min_payout` (base units, decimal string) | Provided. Minimum is in base units because the chain's decimal count is not fixed in this repo. |
| Ticker (`coin`) | S1 required | pool `coin` = `null` | **Not provided.** The repo does not fix a ticker. Owner decision. A listing site will reject a document without one. |
| Stratum or pool URLs | S1 optional `stratum` | none | Not published. The pool's listen address is a bind address, not a public URL. Owner must name the public endpoint before listing. |
| Independent data sources | S2 needs two | none | Two independent sources are required by S2. This node is one source. Listing will need an external explorer or second node operated by someone else. Not something this repo can do. |

## Summary

- The clearest published field list is S1 (MiningBoard). It is a pool
  document, not a coin listing, but its fields are the ones a pool-stats
  page needs.
- S2 is the strictest: two independent sources, a multi-day confidence window,
  and human review. A single-node explorer will not meet it.
- S3 publishes no requirements. The fields it displays are the ones it
  computes for each coin.
- Open items for the owner before submitting anywhere: the ticker, the
  public pool URL, how the block reward is stated, and who runs the second
  independent data source.
