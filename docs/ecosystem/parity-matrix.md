# Parity matrix (draft, 2026-11-12)

Goal (Eric, 2026-11-12): in function and facilities, nothing another chain ships or has on its public roadmap may be missing from Maya2C; the only exception is user and miner count. This draft is the survey behind that goal. It is a literature review, not a measurement: every "ships" cell comes from a public document, and every Maya2C cell is a `file:line` in this tree or the word **none**.

## Search boundaries

- Sources: WebSearch (23 queries, 2026-11-12), project docs and blogs where a search surfaced them, `features.toml`, `docs/master-prompts/next-evolution-gap.md`, `docs/architecture-vision.md`, `docs/adr/`, `docs/invariants.md`.
- Date window: documents dated 2025-01 to 2026-11. Older design docs (Celestia 2024 roadmap, DFINITY 2024 roadmap) are cited only where no newer one surfaced.
- Chains (31): Ethereum, Solana, Sui, Aptos, Cosmos, Polkadot, Cardano, Avalanche, NEAR, Algorand, QRL, Monero, Zcash, Mina, Starknet, Arbitrum, Optimism, zkSync, TON, Tezos, Hedera, Internet Computer (ICP), Celestia, Kaspa, Bitcoin, Chainlink, Filecoin, Arweave, Bittensor, Fetch/ASI, plus Lido/EigenLayer as named sub-rows.
- Excluded: price, TVL and TPS figures (Standing Order 2); vendor-only claims with no second source are marked (single source).
- Evidence tier per cell: P primary (project docs, blog, BIP/EIP/SNIP text, release notes); S secondary (The Block, Blockworks, Messari, Delphi); T tertiary (exchange blogs, aggregator sites). A T-only cell is a lead, not a finding.
- Not found (gaps in the survey itself): Tezos/Hedera 2026 HCS/HTS detail, Arweave HyperBEAM milestones, Fetch/ASI 2026 roadmap, BitVM and OP_CAT 2026 status, Zashi 2026 roadmap. Those rows are marked (thin).

Queue abbreviations: P-1..P-16 = `.claude/plans/q1-90day-roadmap.plan.md`; V3-*, W3-*, W1-* = `D:/Temp/claude-locks/maya2c-jobs.txt`. All URLs accessed 2026-11-12.

## Matrix

### Accounts

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Account abstraction (smart accounts, session keys) | Ethereum EIP-7702 live since Pectra (P); Starknet, zkSync native AA (S); Sui, Aptos (S) | https://www.theblock.co/post/354414 | `features.toml:3095` smart-account RESEARCH/working | P-5 |
| Keyless / passkey / OIDC login | Aptos Keyless AIP-61 live (T); Sui zkLogin + passkeys (S); Avalanche Granite secp256r1 (P) | https://build.avax.network/blog ; https://aptos-intelligence.vercel.app/reports/feature-progress-overview | **none** (only hits are in unrelated ADRs) | gap P-17 |
| Social / guardian recovery | Sui, Argent, Safe (S) | https://members.delphidigital.io/reports/suis-q3-report-sui-stack-and-momentum-picking-up | `features.toml:3432` vault-accounts REAL/verified; ADR-056 guardians | shipped |
| Named / human-readable accounts | NEAR named accounts (T); ENS, SuiNS, TON DNS, Permaweb Names (S) | https://coinpaprika.com/education/near-protocol-explained | **none** | P-3 |
| Sponsored fees / paymaster | Sui, Starknet, zkSync, ERC-4337 (S) | https://members.delphidigital.io/reports/suis-q2-on-chain-ux-primitives-btcfi-expansion-scaling-production | **none** | P-4 |
| Multisig (m-of-n) | all; Algorand plans multi-scheme PQ multisig end-2026 (P) | https://algorand.co/blog/algorand-targets-broad-quantum-resilience-by-2027 | `features.toml:1870` m-of-n ML-DSA multisig REAL/verified; mixed-suite signers **none** | P-16 UI partial (2026-11-14: wallet flow, hybrid approvers only; ADR-117 floor not built); gap P-18 |
| Chain signatures (sign for other chains from one account) | NEAR chain signatures, 11-of-16 MPC (S); ICP threshold ECDSA/Schnorr (P) | https://blockworks.co/news/near-protocol-chain-signatures-multichain-DeFi ; https://medium.com/dfinity/the-internet-computer-roadmap-62487bdce289 | `features.toml:1859` custody-mpc (vault only, inv 18/19); foreign-chain signing **none** | gap P-19 |

### Tokens, NFTs, data carriers

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| User-issued fungible tokens | ERC-20, SPL/Token-2022, Cosmos bank, Hedera HTS, Algorand ASA | https://www.hedera.com/roadmap | `RegisterAsset`/`TransferAsset` exist (`crates/node/src/core/payload.rs:457-460`) with no mint after registration (`crates/node/src/state/asset.rs:17-25`); corrected 2026-11-12 by ADR-103 | P-1 (mint policy) |
| NFTs / collectibles | ERC-721/1155, Metaplex, Sui objects | https://www.coingabbar.com/en/crypto-blogs-details/aptos-blockchain-guide-2026-move-defi-development (T) | **none** | P-2 |
| Confidential token transfers | Solana Token-2022 confidential (T, thin); Zcash NU7 shielded assets planned (S) | https://whale-alert.io/stories/93b80134a9ee8a/ | `features.toml:1837` shielded pool is native coin only | gap P-20 |
| State compression (cNFT, ZK compression) | Solana (T, thin) | https://solanastaking-seven.vercel.app/blog/solana-2026-roadmap-breakdown | **none** | gap P-21 |
| Inscriptions / runes / KRC-20 metaprotocols | Bitcoin ordinals and runes; Kaspa Silverscript covenants (P) | https://kaspa.org/lore | **none** | gap P-22 |
| Cross-chain token standard | Chainlink CCT on CCIP 2.0 (P); LayerZero OFT | https://chain.link/press-releases/chainlink-launches-ccip-2-0-to-maximize-the-utility-and-reach-of-institutional-digital-assets-across-blockchains | **none** | gap P-23 |

### Fees and blockspace

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| EIP-1559-style base fee | Ethereum and most | https://blog.ethereum.org/2026/02/18/protocol-priorities-update-2026 | `features.toml:2066` REAL/verified; ADR-076 clamp | shipped |
| Local fee markets / lanes | Solana per-account priority fees (S) | https://www.blockdaemon.com/blog/solana-in-2026-technical-roadmap | `features.toml:3283` local-fee-markets-lanes RESEARCH/working | promote decision |
| Blockspace reservation / coretime market | Polkadot Agile Coretime live (P) | https://parity.io/blog/polkadot-roundup-2025 | **none** | gap P-24 |
| State rent / storage staking / state expiry | Solana rent, NEAR storage staking (T); Ethereum research | https://coinpaprika.com/education/near-protocol-explained | **none** | gap P-25 |
| Block byte cap | all | (common) | `crates/fee-market/src/limits.rs` MAX_BLOCK_BYTES, inv 33 | shipped |

### Staking, governance, treasury

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| PoS staking and slashing | all PoS | (common) | `features.toml:3338` REAL/verified (PoW + DAG-BFT hybrid, ADR-044) | shipped |
| Liquid staking | Lido, Jito, Stride | roadmap plan | **none** | P-6 |
| Restaking / shared-security export | EigenLayer, Babylon, Cosmos ICS now permissionless (P) | https://forum.cosmos.network/t/cosmos-hub-monthly-community-call-recap/16980 | **none** | gap P-26 |
| On-chain governance lifecycle | Cardano CIP-1694 (P), Polkadot OpenGov, Cosmos | https://www.iog.io/news/what-did-io-deliver-for-cardano-in-q2-2026 | `features.toml:2045` REAL/verified; council `3349` | shipped |
| Vote delegation (DReps) / conviction voting | Cardano DReps (P); Polkadot OpenGov | same | **none** (E4: representative voting needs PoP) | gap P-27 |
| Treasury grants / public goods | Cardano Catalyst + IO treasury (S); Optimism RetroPGF; Polkadot treasury | https://www.theblock.co/news/ecosystems/2026-04-22-input-output-scaled-back-cardano-treasury-plan-leios-mainnet-398484 | `features.toml:3228` treasury-vesting-rules RESEARCH; no spend path (E3/E5) | gap P-28 |
| Scheduled upgrades with sunset | Bitcoin BIP-361 draft (S); Algorand 2027 plan (P) | https://www.spark.money/research/bitcoin-bip-361-quantum-migration-roadmap | `crates/node/src/state/upgrades.rs` inv 34, ADR-082 | shipped |

### Privacy

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Shielded pool | Zcash Orchard + Ironwood pool (S); Monero | https://zeroknowledge.substack.com/p/a-new-chapter-for-zcash ; https://www.kucoin.com/news/flash/zcash-to-launch-ironwood-upgrade-in-late-july-2026-introducing-new-privacy-pool (T) | `features.toml:1837` REAL/verified (opt-in; mainnet v1 off) | C1 decision |
| Full-chain membership proofs / ring signatures | Monero FCMP++ on stressnet, no mainnet date (P: CCS) | https://repo.getmonero.org/monero-project/ccs-proposals/-/merge_requests/668 | **none** (C5); association sets `3454` RESEARCH | not queued; see innovations |
| Stealth addresses | Monero CARROT (S) | same | **none** | V3-P4a/b |
| Viewing keys / selective disclosure | Zcash, Midnight | https://zechub.substack.com/p/zcash-shielded-news-vol57 | `features.toml:3294` pq-viewing-keys; `1933` credentials; both RESEARCH | promote decision |
| Private smart contracts | Midnight testnet (S), Aztec | https://www.essentialcardano.io/development-update/weekly-development-report-as-of-2026-04-17 | `features.toml:3443` private-contract-state RESEARCH | promote decision |
| Confidential cross-chain intents | NEAR Confidential Intents 2026-03 (T) | https://www.kucoin.com/news/articles/near-enhances-cross-chain-privacy-with-confidential-intents-launch | **none** | folded into P-30 |

### Post-quantum cryptography

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| PQ signatures on accounts | QRL Zond ML-DSA-87 testnet, mainnet not confirmed (P); Algorand Falcon-1024 native accounts 2026 (P); NEAR PQ signing 2026-07 (T, single source) | https://www.theqrl.org/2/ ; https://algorand.co/technology/post-quantum ; https://cryptobriefing.com/?p=349560 | `features.toml:1807,1817` ML-DSA, SLH-DSA REAL/verified; hybrid `2941` | shipped |
| PQ KEM transport | not found in surveyed chains | (none) | `features.toml:1786,1796,2148` ML-KEM p2p | shipped |
| PQ state proofs | Algorand Falcon state proofs (P) | https://algorand.co/technology/post-quantum | STARK (hash-based) `2974`; no chain-state proof | gap P-31 |
| PQ VRF | Algorand research target early 2027 (P) | https://algorand.co/blog/algorand-targets-broad-quantum-resilience-by-2027 | `features.toml:1893` VRF is RFC 9381 ECVRF (classical, inv 10) | gap P-32 |
| Legacy-signature sunset | BIP-361 draft (S) | spark.money above | inv 34 suite sunset | shipped |
| PQ address migration tooling | Bitcoin BIP-360 draft, merged 2026-02 (P: bitcoindev list) | https://groups.google.com/g/bitcoindev/c/oQKezDOc4us | `features.toml:3305` quantum-exposure-tool REAL/verified; `3151` migration rehearsal | shipped |

### Interoperability

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Light-client bridge | IBC v2 / Eureka, no multisig trust (P); Ethereum beacon LC | https://docs.cosmos.network/skip-go/eureka/security-properties | `features.toml:3359` eth-beacon-light-client RESEARCH/verified; `3272` interop routes | P-13 |
| Bitcoin bridge | TON Teleport (T), ICP ckBTC (P), Starknet dual settlement (S) | https://www.spark.money/research/bitcoin-starknet-dual-chain-settlement-protocol | `features.toml:2364` btc-lock-mint-bridge RESEARCH; `3029` btc-spv | not queued; gap P-33 (BTC anchoring) |
| Generic messaging (CCIP, Warp/ICM, XCM) | Chainlink CCIP 2.0 (P), Avalanche ICM ACP-118 (P), Polkadot XCM | https://docs.avax.network/blog/etna-changes | **none** native (I2) | gap P-34 |
| Intents + solver network | NEAR Intents (S), Skip Go, CoW | https://blockeden.xyz/blog/2026/01/20/chain-abstraction-near-intents-cross-chain-unified-liquidity-crypto-ux/ (T) | `crates/interop` intents RESEARCH; no solver market | gap P-30 |
| Atomic swaps | many | (common) | `features.toml:1943` HTLC REAL/verified; `1954` lattice HTLC | shipped |

### DA, storage

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Data-availability sampling | Ethereum PeerDAS live 2025-12 (S, Fusaka); Celestia Matcha 128 MB (P) | https://blog.celestia.org/matcha/ ; https://everstake.one/resources/blog/ethereum-glamsterdam-upgrade-explained (T) | `features.toml:3106` data-availability RESEARCH/working | V3-P2 partial; gap P-35 (blob tx) |
| DA attestations to other chains | Celestia Blobstream (P) | https://blog.celestia.org/roadmap/ | **none** | gap P-35 |
| Permanent / paid storage market | Arweave; Filecoin Onchain Cloud + PDP mainnet 2026 (S); Walrus (S) | https://filecoin.io/blog/filecoin-news-120 ; https://messari.io/report/filecoin-solstice-and-the-shift-to-a-demand-driven-service-economy | `crates/archive` (own history only) | gap P-36 |
| Archive + pruning | all | (common) | `features.toml:2024` REAL/verified, inv 26/27 | shipped |

### Scaling

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Parallel execution | Solana, Aptos Block-STM v2 (P), Sui, Monad | https://aptosfoundation.org/currents/technical-alpha-innovations-coming-to-the-aptos-blockchain | `features.toml:3084` parallel-exec RESEARCH | V3-P1 |
| Stateless validation | Ethereum binary tree EIP-7864 (S) | https://www.theblock.co/news/ecosystems/2026-03-01-vitalik-buterin-lays-out-a-two-part-plan-to-overhaul-ethereums-execution-layer-from-the-ground-up-391681 | `features.toml:2128` RESEARCH; activation u64::MAX | V3-P2 |
| Sharding | NEAR Nightshade (T); Polkadot elastic scaling (P) | https://parity.io/blog/polkadot-roundup-2025 | `features.toml:2087,2097` blockgraph RESEARCH | not queued (M4/M5, Eric decision) |
| BlockDAG + fast finality | Kaspa 10 BPS, Toccata 2026-06 (P); Sui Mysticeti; Solana Alpenglow approved, mainnet TBD (S); TON Catchain 2.0 (T) | https://kaspa.org/lore ; https://www.dextools.io/news/solana-firedancer-alpenglow-ecosystem-momentum-june-2026 | `features.toml:3007` dag-bft REAL/verified; finality bench PR #461 | shipped |
| Rollup / L2 framework | OP Stack; Arbitrum Orbit + BoLD (S); ZK Stack Airbender (S); Avalanche L1s ACP-77 (P) | https://www.avax.network/blog/etna-enhancing-the-sovereignty-of-avalanche-l1-networks ; https://blockeden.xyz/blog/2026/04/21/zksync-2026-roadmap-elastic-chain-prover-compression/ (T) | `features.toml:2768` l2-flash REAL/verified; appchains **none** | P-14; gap P-37 |
| Succinct chain-state proof | Mina (P); Cardano Mithril recursive SNARK testnet (P); Starknet S-two (P) | https://minaprotocol.com/blog/road-to-mesa-status-update-march-2026 ; https://www.starknet.io/blog/s-two-is-live-on-starknet-mainnet-the-fastest-prover-for-a-more-private-future/ | **none** | gap P-31 |
| Second validator client | Solana Firedancer live (S) | dextools above | single Rust client; `features.toml:3129` spec conformance | gap P-38 |

### VM and languages

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| WASM VM | NEAR, Polkadot, CosmWasm, ICP | (common) | `features.toml:2014` REAL/verified; tiers `3018` | shipped / V3-P3 |
| EVM compat | hundreds; Tezos X EVM + Michelson on one ledger, testnet 2026-05 (P); Hedera Pectra parity (P) | https://spotlight.tezos.com/tezos-x-from-roadmap-to-reality/ ; https://www.tezos.com/tezos-x/ | `features.toml:3380` multivm-phase-a RESEARCH | P-8 |
| Move / Cairo / multi-language | Sui, Aptos; Arbitrum Stylus with Move compiler 2026-02 (T) | https://blockeden.xyz/blog/2026/02/01/arbitrum-2026-roadmap-arbos-dia-gaming-catalyst-stylus/ | Rust-to-WASM only | covered by P-8 phase B |
| Contract timers / scheduled execution | ICP timers (P), Hedera scheduled tx (P) | https://www.hedera.com/roadmap | **none** (P-7 is payments only) | gap P-39 |
| Randomness host primitive | ICP raw_rand, Chainlink VRF | (common) | VRF `1893` not exposed to contracts | gap P-40 |
| Threshold decryption primitive | ICP vetKeys completed 2025-07 (P) | https://legacy.internetcomputer.org/roadmap | `features.toml:1903` sealed mempool only | gap P-41 |
| HTTPS outcalls | ICP (P) | same | **none**; collides with inv 24 and 11 | gap P-42 (design-only) |
| Formal verification | Cardano (Agda/Lean), Mina | (common) | `features.toml:2489` Kani REAL/verified; Lean `2758` planned | V3-P5 |

### Dev tooling, wallets, explorer

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| SDKs (TS/Py/Go/Rust/WASM) | all | (common) | `features.toml:2715` REAL/verified; C++ **none** (D1) | gap P-43 |
| Local devnet, replay debugger | Solana test validator; Hedera Solo (P) | https://www.hedera.com/roadmap | `features.toml:3421` maya2c-dev-cli REAL | shipped |
| Indexer / subgraphs | The Graph, Helius | (common) | explorer `2695`; #414 API | P-15 |
| Event subscriptions / webhooks | Helius, Alchemy, The Graph substreams | (common) | `features.toml:2705` REST/GraphQL; subscriptions **none** | gap P-44 |
| Desktop wallet | all | (common) | `features.toml:2685` REAL/verified | shipped |
| Browser-extension wallet | MetaMask, Phantom | (common) | **none** | P-9 |
| Mobile wallet / light client | all; TON in Telegram, mini-apps (T) | https://dev.to/ton-adoption/ton-roadmap-2026-2027-what-developers-are-promising-5ec6 | **none**; chat `2801` RESEARCH | P-10; gap P-45 (chat mini-apps) |
| Hardware wallet | Ledger | (common) | `features.toml:2847` working | P-11 |
| Signed update channel | most apps | (common) | `apps/app-update`, ADR-075 | shipped |

### Oracles, identity, AI

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Push price feeds | Chainlink Data Feeds (P), Pyth | https://chain.link/blog/quarterly-review-q3-2026 | `crates/eo-oracle` RESEARCH (NDVI only) | P-12 |
| Pull / low-latency streams | Chainlink Data Streams incl. US equities (P); Pyth pull | https://chain.link/press-releases/chainlink-launches-data-streams-for-u-s-equities-and-etfs-to-power-secure-tokenized-rwa-markets-onchain | **none** | gap P-46 |
| DID / credentials | ICP Internet Identity (P), Polygon ID | ICP roadmap above | `features.toml:1913,1923,1933` RESEARCH | promote decision |
| Proof of personhood | World; not found on surveyed L1s natively | (none) | `features.toml:2353` RESEARCH (no liveness) | not queued (ID4, Eric decision) |
| Compliance proofs / KYC | Midnight; Zcash viewing | (common) | `features.toml:2613,3390` RESEARCH | promote decision |
| AI inference market / subnets | Bittensor dTAO subnets, roadmap to late 2027 (S) | https://cryptobriefing.com/bittensor-full-stack-intelligence-network-roadmap/ ; https://www.falconx.io/newsroom/state-of-bittensor-subnet-adoption-trends-network-mechanics-and-covenants-departure | `features.toml:2332` verifiable-compute-job-market RESEARCH/planned; ADR-079 | gap P-47 (ADR, no job) |
| Agent framework and agent payments | Fetch/ASI Agentverse (T, thin); Chainlink for Agents (P) | https://chain.link/news ; https://blockeden.xyz/blog/2025/09/21/bittensor-tao-decentralized-ai/ (T) | `features.toml:3261` mcp-server REAL; agent payments **none** (A5) | gap P-48 |
| AI debug assistant | not found in surveyed chains | (none) | `bins/maya2c-mcp` + replay | V3-P6 |

### MEV and ordering

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Encrypted mempool | Shutter; ICP vetKeys MEV use (P) | ICP roadmap above | `features.toml:1903` REAL/verified, ADR-025 | shipped |
| ePBS / inclusion lists (FOCIL) | Ethereum Glamsterdam EIP-7732 running late; Hegota FOCIL (P + T) | https://blog.ethereum.org/2026/02/18/protocol-priorities-update-2026 ; https://blockeden.xyz/blog/2026/04/18/glamsterdam-epbs-delay-ethereum-mev-reform-slip/ | **none** fair-ordering rule (K6) | gap P-49 |
| Express-lane auction | Arbitrum Timeboost (T) | blockeden Arbitrum above | **none**; ADR-025 position is sealed, not auctioned | not a gap by policy |

### Upgrades

| Facility | Who | Source | Maya2C | Queue |
|---|---|---|---|---|
| Forkless runtime upgrade | Polkadot; Cosmos x/upgrade (P) | https://docs.cosmos.network/sdk/next/upgrade/release | `features.toml:3140` protocol-upgrade-halt; ADR-082 levers; hot-patcher `2561` planned (inv 13 collision) | shipped (halt-before-fork model) |
| Reproducible builds + provenance | Ethereum clients, Bitcoin Guix | (common) | ADR-080; `features.toml:2500` supply-chain gates | shipped |

## Gap list (new job ids)

Each gap: no code in the tree, and no P-1..P-16, V3-*, W1-* or W3-* job covers it. Size S/M/L as in the gap table; consensus y/n; invariants it would touch (`docs/invariants.md`).

| Job | Gap | Size | Consensus | Invariants | Note |
|---|---|---|---|---|---|
| P-17 | Passkey / device-authenticator login that unlocks a local PQ key (wallet-side; no on-chain P-256 suite) | M | n | none in wallet-only form; 29, 30 if a suite were added | the passkey never signs a transaction, it unlocks the ML-DSA key |
| P-18 | ~~Mixed-suite multisig signers~~ already shipped: `crates/crypto-pq/src/multisig.rs:101-105` (per-member suite), tests `multisig_tests.rs:38-45`; ADR-117 adds only a per-family approval floor | S | y | 29, 30, 31 | v12 (floor only) |
| P-19 | Chain signatures: MPC signing for foreign chains from a Maya2C account | L | n (service) | 11, 18, 19 | disclosed operator set; ADR first |
| P-20 | Shielded transfers for P-1 tokens | L | y | 25, 31, 32 | after P-1 and the C1 decision |
| P-21 | State compression for mass NFT/account issuance | L | y | 24, 25 | after P-2 |
| P-22 | Data-carrier field + indexer convention (inscription-class metaprotocols) | S | y | 31, 33 | wire-format change; byte cap applies |
| P-23 | Cross-chain token standard (burn/mint authority over P-13 routes) | M | y | 11, 25, 31 | after P-1, P-13 |
| P-24 | Blockspace reservation / lane futures | M | y | 17, 33 | builds on local-fee-markets-lanes |
| P-25 | State rent or storage staking | M | y | 17, 25, 27 | Eric: economics |
| P-26 | Shared-security export / restaking | L | y | 12, 17, 32 | Eric: economics; ADR-016 scope |
| P-27 | Vote delegation (DRep-style) and conviction voting | M | y | 12, 13, 17 | stake-weighted only until PoP |
| P-28 | Treasury spend path + public-goods grants round | M | y | 12, 13, 17, 32 | Eric: money |
| P-29 | (reserved; merged into P-30) | - | - | - | |
| P-30 | Intent solver market with HTLC settlement; optional confidential intents | M | n | 11, 31 | off-chain solvers, on-chain HTLC |
| P-31 | Succinct chain-state proof (recursive STARK over checkpoints) for light clients and bridges | L | n, then y | 24, 26 | prover off-chain first |
| P-32 | PQ VRF (hash- or lattice-based) replacing ECVRF in committee selection | L | y | 10, 12, 29, 30 | new cryptography; peer review first |
| P-33 | Bitcoin anchoring of Maya2C checkpoints (publisher + verifier) | M | n | 11, 26 | publisher is a disclosed party |
| P-34 | Generic cross-chain messaging over verified headers (CCIP/ICM-class) | L | y | 11, 24, 25 | after P-13 |
| P-35 | Blob transactions with expiry; DA attestations exportable to L2s | L | y | 25, 27, 31, 33 | needs its own fee lane |
| P-36 | Paid storage market with possession proofs | L | y | 11, 25, 26, 27 | Eric: storage economics |
| P-37 | Appchain framework: a chain secured by Maya2C validators | L | y | 12, 24, 25 | ADR-016 scope question |
| P-38 | Second independent client, or a differential conformance harness against the spec | L | n | 24 | start with the harness |
| P-39 | Contract timers / scheduled execution (general, not payments) | M | y | 21, 24, 25 | fuel charged at schedule time |
| P-40 | VRF host function for contracts | S | y | 10, 21 | exposes `features.toml:1893` |
| P-41 | Threshold-decryption host primitive over the sealed-mempool committee | M | y | 11, 21 | committee stays optional |
| P-42 | Attested off-chain fetch (HTTPS-outcall class) design study | S (design) | y if built | 9, 11, 24 | likely rejected: non-determinism |
| P-43 | C++ SDK wrapper over sdk-ffi | S | n | none | D1 |
| P-44 | Event subscriptions / webhooks on the gateway | S | n | none | |
| P-45 | Chat mini-apps and in-chat wallet (TON-class) | M | n | none | on maya-chat |
| P-46 | Pull oracle: signed report verified on chain; low-latency streams | M | y | 9, 11, 17 | after P-12 |
| P-47 | Verifiable-compute job market (ADR-079 to job) + AI inference receipts via WorkClaim | L | y | 11, 13, 21 | ADR exists, no job |
| P-48 | Agent registry + agent-to-agent payments (session keys, signed receipts) | M | n | 31, 32 | A5 |
| P-49 | Inclusion lists / fair-ordering rule for proposers | L | y | 11, 24 | K6; two human reviewers |

Gaps found: **32** new job ids (P-17 to P-49, P-29 reserved); 20 touch consensus.

## Innovations

Facilities in this tree, or proposed here, that were **not found in the surveyed chains as of 2026-11-12**. Each needs `docs/prior-art/<feature>.md` before any copy uses stronger wording (Standing Order 10).

Already in the tree:
1. Genesis-committed suite sunset schedule (inv 34, ADR-082). BIP-361 and Algorand describe plans; a shipped, tested schedule was not found.
2. ML-KEM-1024 PQ-Noise peer transport (`features.toml:2148`). Not found in the surveyed chains' docs.
3. Lattice HTLC atomic swaps (`features.toml:1954`) and PQ viewing keys (`features.toml:3294`).
4. Quantum-exposure tool for other chains' outputs (`features.toml:3305`).
5. Hash-migration rehearsal (`features.toml:3151`).
6. Threshold-encrypted mempool with PQ suites (`features.toml:1903`). Shutter and ICP vetKeys are classical.
7. Reality ledger gated in CI (`features.toml`, ADR-004) and the trust-facts sheet (W3-8).
8. Sentinel that proposes and never acts (ADR-077, ADR-101).
9. Undo-send and payee-check in the wallet (#460).
10. Lattice proof-of-useful-work (`features.toml:2107`, RESEARCH).

Proposed, not in the tree:
11. PQ chain signatures (P-19 with ML-DSA). NEAR and ICP sign ECDSA/Schnorr only.
12. PQ VRF in committee selection (P-32). Algorand lists it as 2027 research.
13. Passkey-unlocked PQ key (P-17). Surveyed passkey schemes put P-256 on chain.
14. Mining-to-inference: GPU miners earn through verified inference WorkClaims (P-47, ADR-044 kind). Bittensor rewards are validator-scored, not proof-verified.
15. Shielded P-1 tokens with PQ viewing keys (P-20). Zcash NU7 shielded assets are planned and classical.
16. Deterministic replay as the AI debugger's only input (V3-P6).

Items 11 to 16 are claims about this survey, not about the world; a dated prior-art search is still required before shipping copy.

## Discrepancies and thin rows

- Ethereum Glamsterdam date: sources give H1 2026, Q3 2026 and late 2026; none primary. Treated as planned.
- Solana Alpenglow: approved 2025-09 by stake vote (S); mainnet date unconfirmed.
- QRL 2.0 mainnet: not confirmed by any source as of 2026-07.
- Monero FCMP++: stressnet only; one T source claims a Q1 2026 fork, contradicted by the project's own CCS pages.
- Mina Mesa: one Mina post (2026-09-08) says landed; a Q2 post says schedule pending. Unresolved.
- Hedera HCS/HTS, Tezos X mainnet, Arweave HyperBEAM, ASI-1, BitVM, OP_CAT: thin or not found.
