# Beat bars

Master Prompt 21 §3. One measurable bar per weakness: the best public number
found, with its source, and Maya2C's number measured by the method in
`docs/BENCHMARK_METHODOLOGY.md`. **A bar is "beaten" only when Maya2C's
number is measured and reproduced by someone outside the project. None is.**

| Weakness | Bar | Best public number (source) | Maya2C today | Beaten? |
|---|---|---|---|---|
| Finality | p99 time to irreversibility | Ethereum ~15 min ([ethereum.org](https://ethereum.org/roadmap/single-slot-finality/)); faster chains not researched | probabilistic PoW: 24 confirmations = 6 min at q = 0.3, P < 10⁻³ (`docs/integrations/EXCHANGES.md`) | no |
| Quantum readiness | share of value behind PQ signatures from genesis | QRL: 100 % since 2018 ([QRL docs](https://docs.theqrl.org/what-is-qrl/)) | 100 % hybrid from genesis | **ties QRL; not a lead** |
| Node cost | disk for a full node | Ethereum ~1–1.3 TB execution client in 2026 after history expiry ([7BlockLabs](https://www.7blocklabs.com/blog/ethereum-full-node-disk-size-2026-ethereum-full-node-storage-requirements-2026-and-ethereum-full-node-size-2026)) | 3.4 TB/month at 100 TPS of PQ bodies, unpruned (`docs/NODE_TYPES.md`) | **no: worse** |
| Liveness | longest halt in the last 24 months | Solana ~5 h, Feb 2024 ([Helius](https://www.helius.dev/blog/solana-outages-complete-history)) | no public network | not measurable |
| Blind signing | share of transactions a wallet renders in plain language | not researched | transfers only (Ledger app fields) | no |
| New-user time to first transfer | minutes, from nothing | not researched | not measured (no study) | no |
| Bridge trust | parties that must be honest for a cross-chain transfer | Ronin: 5 of 9 keys ([Halborn](https://www.halborn.com/blog/post/explained-the-ronin-hack-march-2022)) | no bridge; Bitcoin SPV verifies headers with PoW, but nothing moves value | no |
| Exploit losses | value lost per value secured | $3.4B stolen in 2025, all chains ([Chainalysis](https://www.chainalysis.com/blog/crypto-hacking-stolen-funds-2026/)) | no value secured | not measurable |
| Developer time to a tested contract | minutes | not researched | cold build 4 m 54 s; no template/deploy path (`reports/17-integrations.md`) | no |

The honest summary: Maya2C **ties** the best existing post-quantum chain on
quantum readiness, is **worse** on node cost because of PQ signature size,
and has no public network on which to measure the rest.
