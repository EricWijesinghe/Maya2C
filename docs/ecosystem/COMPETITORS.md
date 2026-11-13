# Competitor comparison, 2026-11-10

Where Maya2C leads, ties and trails the chains it is compared with, and what
closes each trailing cell. Owner's standard (2026-11-10): "except for miner
count, no blockchain should come close". This page is the honest measure of
how far that is, so the build order follows it.

## Rules this page follows

- **Competitor facts** carry a source ID `[S#]` with a URL and an access date.
  Facts marked `(prior-art, 2026-11-01)` were taken from
  [docs/prior-art/pq-l1-comparison-2026-11.md](../prior-art/pq-l1-comparison-2026-11.md),
  whose sources were accessed 2026-11-01 and were not re-fetched today.
- **Maya2C facts** cite a repo file, test, ADR or report. Maya2C numbers come
  solely from `reports/`. Every Maya2C throughput number is **single-machine,
  localhost, 4 validators, one shared Windows PC**; no multi-machine run exists
  (CLAUDE.md, Standing Orders 1 and 2).
- **Cells marked `n/a`** mean the fact was not verified in this session. They
  are not "trail" and not "lead". They are listed as gaps in section 4.
- **Verdict key.** `Lead` = Maya2C is ahead on the stated fact. `Par` = roughly
  level, or both lack the feature. `Trail` = a competitor is ahead today. `n/a`
  = not verified. A `Lead` on a design is labelled as design: Maya2C has no
  mainnet yet, so a live feature elsewhere beats an in-tree one here.
- Wording follows Standing Order 10 (no superlative novelty claims).

## 1. Maya2C baseline, with evidence

| Dimension | Maya2C state | Evidence |
|---|---|---|
| PQ signatures on every default transaction | Hybrid ML-DSA-65 + SLH-DSA-SHA2-128s; both must verify | `crates/node/src/core/transaction.rs:360` (`Transaction::verify`); invariant 31 in `docs/invariants.md`; `features.toml` `hybrid-signature-transaction-validation` = verified, REAL |
| PQ transport | ML-KEM-1024, ML-KEM-768 fallback, layered above libp2p Noise | `features.toml` `p2p-mlkem-1024` = working, "not externally reviewed"; `docs/pq-transport.md` |
| Crypto-agility | Signature-suite registry and sunset record; scheduled upgrades under compiled leads | invariants 29, 30 and 34; ADR-082 |
| Finality | DAG-BFT (Narwhal + Bullshark) | `features.toml` `dag-bft` = verified, REAL; ADR-027 |
| Measured finality or throughput | **Throughput, not per-transaction finality latency.** 4000 transfers finalized and visible on a replica in 9.68 s over 16 blocks; 413.3 finalized transfers/s. Mix: 1000 senders, single-output transfers of 1 base unit, hybrid signatures. Commit b6b5f536, Windows, 24 logical cores. Localnet, 4 validators, one machine. Not measured: contract calls, mixed loads, WAN latency, separate machines | `reports/bench/tps-1794192095.md`; second run 3000 senders, 390.2/s, `reports/bench/tps-1794191941.md` |
| Amount width and precision | 256-bit amounts, 24 decimals (`COIN_DECIMALS = 24`) | `crates/amount/src/lib.rs:52`; `features.toml` `amount-256` = working ("for the proved crates"); ADR-046, ADR-068 |
| Formal verification | Kani model checking on `maya-ledger-math`, `maya-dex`, `maya-governance` | `features.toml` `kani-model-checking` = verified, REAL; gate `cargo kani -p maya-ledger-math -p maya-dex -p maya-governance` |
| Determinism | No floats in consensus rules; apply path refuses a state root that execution does not reproduce | CLAUDE.md Standing Order 4 and Execution Directive 2; invariant 24. **Note: CLAUDE.md cites invariant 20 for the no-float rule, but `docs/invariants.md` marks 20 as retired (zkML). The citation needs fixing; see section 5.** |
| Address format | Bech32m text with a network prefix and checksum | ADR-081 |
| Light clients | SPV light client verified; stateless transfer verification is RESEARCH | `features.toml` `spv-light-client` = verified, REAL (`crates/light-client`); `stateless-transfer-verification` = working, RESEARCH |
| Privacy | Shielded pool is **off** at mainnet v1 | ADR-037; `crates/node/tests/shielded_tests.rs` exists, pool disabled on mainnet v1 |
| Validators and nodes | Testnet: 4 validators on one machine. Launch is a disclosed single machine | CLAUDE.md (single machine, localhost); `docs/ecosystem/CATALOG.md` section G, "Gaps, stated plainly" |
| Independent audit | None | `docs/ecosystem/CATALOG.md` section G ("there is no independent audit yet"); `features.toml` marks `p2p-mlkem-1024` and `maya-chat` "not externally reviewed" / "unaudited" |
| Years live | Testnet; no mainnet yet | `docs/ecosystem/CATALOG.md` section G ("no time in production") |
| Ecosystem | No third-party apps yet | `docs/ecosystem/CATALOG.md` section G |

## 2. Competitor facts, sourced

Access date for every `[S#]` below is **2026-11-10** unless a row says
otherwise. Where a source is an aggregator or press release, the row says so.

| Chain | PQ signatures and migration | Finality | Validators and nodes | Track record, audits, privacy |
|---|---|---|---|---|
| Bitcoin | No PQ signatures. BIP-360 (P2MR) is a Draft; spec says it "does not include the introduction of post-quantum signature schemes" (prior-art, 2026-11-01) | Probabilistic, PoW (n/a: not verified this session) | About 23,189 public nodes per Coin Dance as quoted in search; trackers conflict, and Bitnodes went dark 2026-05-03 [S3] | n/a. Transparent; no privacy layer in base chain |
| Ethereum | None live. leanXMSS planned for consensus; Falcon, ML-DSA and SPHINCS+ "being evaluated" for execution; EIP-8355 Draft, no fork scheduled (prior-art, 2026-11-01) | About 2 epochs, about 12.8 min [S2] | About 952,782 active validators as of 2026-03-06 per Glassnode definition [S1] | n/a (track record not verified) |
| Solana | Winternitz vault, opt-in, "not a default feature"; mainnet deployment unconfirmed (prior-art, 2026-11-01) | About 12.8 s, one source; Alpenglow targets about 150 ms, activation unconfirmed [S4] | Sources conflict: about 800 active (Jan 2026) to about 1,414 voting (Apr 2026) [S4] | Mainnet over six years, epoch 1000 [S4b] |
| Sui | ML-DSA-65 accounts opt-in; devnet v1.82.0 on 2026-10-12; mainnet accounts Q1 2027 target (prior-art, 2026-11-01) | Mysticeti: about 390 ms consensus latency at mainnet launch (press release, Aug 2024) [S5]; official docs call their benchmark about 0.5 s and say it is not a production metric [S5b] | About 135 to 137 validators, aggregator figure [S5c] | n/a |
| Aptos | SLH-DSA-SHA2-128s proposal (AIP-137), awaiting acceptance (prior-art, 2026-11-01) | n/a | n/a | n/a |
| Algorand | Falcon accounts live on mainnet, opt-in, August 2026; Falcon state proofs (prior-art, 2026-11-01) | Sub-3 s blocks, instant finality per algorand.co [S6] | About 1,600 validators, secondary report [S6b] | n/a |
| QRL (1.x and Zond) | XMSS, mandatory on mainnet since 2018 (prior-art, 2026-11-01, QRI grade A). Zond on ML-DSA-87 testnet; mainnet gated on audits, "50% complete" (2026-06-26) (prior-art) | PoW, no finality layer stated (prior-art) | n/a | Mainnet since 2018; audit progress as above |
| Cardano | Ed25519 stays; CIP-0197 in review since 2026-08-23, "not yet implemented" (prior-art, 2026-11-01) | Ouroboros: n/a | About 2,900 validators, secondary report [S7] | n/a |
| Polkadot | n/a (not verified) | n/a | About 600 validator slots (a cap, not the active count), per CoinStats [S7b] | n/a |
| Monero | n/a for PQ | n/a | n/a | Privacy mandatory by design: ring signatures, stealth addresses, RingCT, Dandelion++ (secondary sources; official docs not reached) [S8] |
| Zcash | n/a for PQ | n/a | n/a | Privacy optional via zk-SNARK shielded pool, with viewing keys (secondary sources) [S8] |
| Mina | n/a for PQ | n/a | n/a | Mainnet 2021-03-23 [S9]; constant-size chain state of about 22 KB via recursive proofs (Pickles) [S9b] |
| Celestia | n/a | n/a | No reliable active count found [S10] | n/a |

Source list:

- **[S1]** Glassnode, Ethereum active validators definition, https://studio.glassnode.com/charts/eth2.ActiveValidatorsCount?a=ETH (accessed 2026-11-10). Count quoted from search summary; not re-read on the page.
- **[S2]** Prysm, Finality on Ethereum, https://hackmd.io/@prysmaticlabs/finality (accessed 2026-11-10). Also ethereum.org, cited in the same search.
- **[S3]** Protos, Bitcoin node counters go dark after Bitnodes expiry, https://protos.com/bitcoin-node-counters-go-dark-following-bitnodes-expiry/ (accessed 2026-11-10). Node count itself from search summary; the Coin Dance page was not fetched.
- **[S4]** Solana mainnet epoch 1000, https://solanacompass.com/news/solana-mainnet-reaches-epoch-1000-after-more-than-six-years-of-continuous-operation (accessed 2026-11-10). **[S4b]** same page for track record. Finality and validator counts from search summary, https://tatum.io/blog/solana-alpenglow (accessed 2026-11-10). Counts conflict across sources.
- **[S5]** Sui, Mysticeti press release, https://thedefiant.io/news/press-releases/sui-sets-the-standard-for-blockchain-speed-with-new-mainnet-consensus-mechanism (accessed 2026-11-10). **[S5b]** Sui docs, consensus architecture, https://docs.sui.io/learn/architecture/consensus (accessed 2026-11-10). **[S5c]** Aggregator, https://api.emergentmind.com/topics/mysticeti (accessed 2026-11-10), not official.
- **[S6]** Algorand, pure proof-of-stake, https://algorand.co/technology/pure-proof-of-stake (accessed 2026-11-10). Sub-3 s figure from search summary. Validator count from **[S6b]** TradersUnion, https://tradersunion.com/news/cryptocurrency-news/show/2058720-ethereum-dominates/ (accessed 2026-11-10), secondary.
- **[S7]** Same TradersUnion article (accessed 2026-11-10), secondary; Cardano and Algorand counts. Token Terminal data as cited there.
- **[S7b]** CoinStats, Polkadot referendum 1966, https://coinstats.app/news/40c51b3dac9a071e748662419d1d9128287ecf41b792b88a7daa925f6aecf08a_Polkadot-today-Referendum-1966-would-cut-100-of-600-validator-slots/ (accessed 2026-11-10), secondary.
- **[S8]** Monero vs Zcash comparison, https://www.monero.how/monero-vs-zcash-vs-dash-privacy-comparison (accessed 2026-11-10), secondary. Official docs (getmonero.org, zcash.readthedocs.io) not reached, so treat privacy cells as provisional.
- **[S9]** The Block, Mina mainnet launch, https://www.theblock.co/post/99081/mina-protocol-mainnet-launch-zero-knowledge-proof-blockchain (accessed 2026-11-10). **[S9b]** Mina, Coda relaunch as Mina, https://minaprotocol.com/blog/coda-protocol-relaunches-as-mina-the-worlds-lightest-blockchain (accessed 2026-11-10). Sources disagree on the 22 KB figure (one 2022 article says 11 kB).
- **[S10]** Celestia validators, searched 2026-11-10, no reliable source. Left as `n/a`.
- **[S11]** Bitcoin Wiki, Units, https://en.bitcoin.it/wiki/Units (accessed 2026-11-10). 1 satoshi = 10^-8 BTC, 8 decimal places.
- **[S12]** ethereum.org, Intro to ether, https://ethereum.org/en/developers/docs/intro-to-ether/ (accessed 2026-11-10). Wei is 10^-18 ether, 18 decimal places.
- **[S13]** Polkadot Wiki, Learn DOT, https://wiki.polkadot.com/docs/learn-DOT (accessed 2026-11-10). Planck is the smallest unit, 10 decimal places. Page gives no validator count, so DOT validator figures stay secondary.
- **[S14]** Aptos glossary, https://aptos.dev/en/network/glossary (accessed 2026-11-10). Transaction final at commit; typical block times tens of ms; user-visible latency typically sub-second. Octa unit quoted as "1 APT = 10^8 Octas", read from a page that lost its superscript; treated as unverified.
- **[S15]** Mina docs, https://docs.minaprotocol.com/ (accessed 2026-11-10). Describes Mina as "a layer-1 blockchain with a 22KB blockchain". Recursive-proof wording not on this page.
- **Fetch failures this session (accessed 2026-11-10):** solana.com/docs/terminology (429), dev.algorand.co (no decimals or finality figure), getmonero.org atomic units (404), zips.z.cash protocol PDF (unreadable), docs.sui.io tokenomics (no MIST figure), docs.cardano.org roadmap (404), docs.celestia.org glossary (404), github QRL Zond tokenomics (404).
- **Prior-art sources** (accessed 2026-11-01): see [pq-l1-comparison-2026-11.md](../prior-art/pq-l1-comparison-2026-11.md) for QRL, Algorand, Ethereum, Bitcoin, Solana, Cardano, Aptos, Sui and the rest, each with its own URL.

## 3. Verdict matrix

Rows are dimensions, columns are chains. Each cell is Maya2C against that chain.

| Dimension | BTC | ETH | SOL | SUI | APT | ALGO | QRL | ADA | DOT | XMR | ZEC | MINA | TIA |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| PQ signatures live on mainnet | Par | Par | Par | Par | Par | Trail | Trail | Par | n/a | n/a | n/a | n/a | n/a |
| Two PQ families required per default tx | Lead | Lead | Lead | Lead | Lead | Lead | Lead | Lead | n/a | n/a | n/a | n/a | n/a |
| Upgrade and crypto-agility mechanism | Par | Par | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Finality, measured latency | n/a | n/a | n/a | Trail | Trail | Trail | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Amount width and precision | Lead | Lead | n/a | n/a | n/a | n/a | n/a | n/a | Lead | n/a | n/a | n/a | n/a |
| Formal verification coverage | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Determinism rules written and tested | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Address format | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Light clients and succinct state | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | Trail | n/a |
| Privacy | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | Trail | Trail | n/a | n/a |
| Decentralization (validators and nodes) | Trail | Trail | Trail | Trail | n/a | Trail | n/a | Trail | Trail | n/a | n/a | n/a | n/a |
| Independent audit | n/a | n/a | n/a | n/a | n/a | n/a | Trail | n/a | n/a | n/a | n/a | n/a | n/a |
| Years live and track record | n/a | n/a | Trail | n/a | n/a | n/a | Trail | n/a | n/a | n/a | n/a | Trail | n/a |
| Ecosystem size | n/a | Trail | Trail | Trail | Trail | n/a | n/a | Trail | n/a | n/a | n/a | n/a | n/a |

Notes on the matrix:

- **PQ live: Par, not Lead.** Maya2C has no mainnet. Chains with no live PQ
  path are level with it on the mainnet fact; chains with a live opt-in PQ
  path (Algorand, QRL) are ahead.
- **Two PQ families: Lead on design.** No chain in the table requires two PQ
  families on the default path. This is a design comparison, not a live one.
- **Decentralization: every chain with a public count is ahead.** Four
  validators on one machine is behind any figure above.
- **Amount width.** Lead against BTC (8 decimals, [S11]), ETH (18, [S12]) and
  DOT (10, [S13]) on decimal places only. This is a precision comparison; it
  says nothing about value or safety.
- **Finality.** Aptos states that a transaction is final at commit and
  user-visible latency is sub-second ([S14]). Maya2C has no per-transaction
  latency measurement, so it trails there. Lead is not claimed on finality.
- **`n/a` means unfetched.** Every `n/a` cell is a primary source the session
  could not reach: a 404, a 429 rate limit, a redirect not followed, or an
  unreadable PDF (Solana, Algorand, Monero, Zcash, Sui, Cardano, Celestia, QRL
  Zond). Those rows are open work, not a verdict. A Maya2C throughput figure is not a finality latency, so
  it does not count as a Lead.

## 4. Where we trail and what closes it

Each Trail cell maps to a row in [CATALOG.md](CATALOG.md). Where no row exists,
a NEW row is proposed in that section, with the wave and the real paths it
builds on. NEW rows are added to CATALOG in the same change.

| Trail cell | Closed by | Notes |
|---|---|---|
| Decentralization: validator count, geography (ETH, SOL, SUI, ALGO, ADA, DOT, BTC) | **NEW** `validator-recruitment` (section D, W1). Also `node-in-a-box` (existing, W1) and `validator-console` (existing, W2) | Recruiting outside validators conflicts with the owner's 2026-10-28 directive (no third-party validators before mainnet). Gate: Eric's decision on timing. Tooling is buildable now |
| Independent audit (QRL is ahead on audit progress) | **NEW** `independent-audit-program` (section D, W1) | Gate: Eric approval and a budget. The self pre-audit is not an audit. Builds on `features.toml` `kani-model-checking` and `crates/build-guard` |
| Years live and track record (SOL, QRL, MINA) | No catalog row can close it. Time, and the mainnet gate | Stated as a gap in CATALOG section G |
| Ecosystem size (ETH, SOL, SUI, APT, ADA) | Existing: `sdks-1.0`, `dev-portal`, `testnet-kit`, `merchant-checkout` (all W1) | Ecosystem size is a result of these rows, not a row by itself |
| Measured finality latency (SUI, APT, ALGO) | **NEW** `finality-latency-bench` (section D, W0, size S). Measures per-transaction time from submit to finalized, with a stated mix and hardware | Non-consensus. Builds on `cargo xtask bench-tps` and `reports/bench/`. Must be run and reported with the mix, hardware and command (Standing Order 2) |
| Privacy (XMR, ZEC; shielded pool is off) | **NEW** `shielded-pool-enable` (section B, W3) | Gate: an independent audit of the join-split circuit, then an activation height (ADR-002, ADR-037). Builds on `crates/node/tests/shielded_tests.rs` |
| Succinct light client (MINA) | **NEW** `succinct-state-proofs` (section D, W4) | Research and an ADR come before code. Builds on `crates/stateless-core` and `crates/light-client`, which is verified as SPV |
| PQ signatures live (ALGO, QRL) | No catalog row. Closed by the mainnet gate | Mainnet launch is the closing item, not an app |

## 5. Items for Eric, and fixes this page needs

1. **Invariant citation.** CLAUDE.md says "no float enters a consensus rule
   (invariant 20)". `docs/invariants.md` marks invariant 20 as retired (zkML,
   ADR-008). Either the citation moves to invariant 24, or invariant 20 is
   restored with a test. Resolved 2026-11-14 (founder audit Q20 A): the rule
   is invariant 36, pinned by `cargo xtask float-check`.
2. **Competitor sources.** About half the competitor cells come from
   aggregators, press releases or secondary blogs, and some sources disagree
   (Solana validators, Bitcoin nodes, Algorand finality). Before this page is
   quoted externally, the cells marked `n/a` need a primary source.
3. **Monero and Zcash privacy cells** rest on secondary sources. Official docs
   were not reached.
4. This page compares an unlaunched testnet with live chains. The honest
   reading is in section 3: the gaps are time, validator count, audit and
   ecosystem, and they are not closed by any feature in the tree.
