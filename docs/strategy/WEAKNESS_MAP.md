# Weakness map

Master Prompt 21 §1. Problems the large chains openly struggle with, with
dated public sources, why legacy makes each hard for them, and what in Maya2C
addresses it. **Researched 2026-09-26** by web search. The sources are
linked; figures are the sources' own.

> **Coverage.** Bitcoin, Ethereum (L1), Solana, Sui and QRL were
> researched in this pass. Ethereum L2s, Aptos, Cosmos, Polkadot, Avalanche
> and TON were **not**. Their rows are absent rather than filled from
> memory.

| # | Weakness | Evidence (dated, linked) | Why legacy makes it hard | Maya2C component | Maya2C status |
|---|---|---|---|---|---|
| 1 | **Blind signing** | Bybit, 21 Feb 2025, ~$1.5B: signers approved a malicious Safe upgrade their hardware wallets could not parse ([NCC Group](https://www.nccgroup.com/research/in-depth-technical-analysis-of-the-bybit-hack/), [CoinDesk](https://www.coindesk.com/business/2025/02/26/bybit-and-safe-custody-blame-each-other-over-usd1-5b-hack)) | arbitrary calldata to arbitrary contracts; clear signing needs every dApp's metadata | transaction intent standard + mandatory simulation (Master Prompt 22) | not built; the Ledger app shows transfer fields (`apps/ledger-maya2c`) |
| 2 | **Theft at scale** | $3.4B stolen in 2025; Bybit alone 69 % of H1 ([Chainalysis](https://www.chainalysis.com/blog/crypto-hacking-stolen-funds-2026/), [H1 update](https://www.chainalysis.com/blog/2025-crypto-crime-mid-year-update/)) | custody built on EOA keys and multisig UIs | smart accounts with vault delays and guardians (`crates/smart-account`) | RESEARCH: tested, not in the transaction format |
| 3 | **Bridge security** | Ronin, Mar 2022, ~$624M: 5 of 9 validator keys taken ([Halborn](https://www.halborn.com/blog/post/explained-the-ronin-hack-march-2022)); Wormhole, Feb 2022, ~$326M: signature verification bypassed ([Halborn](https://www.halborn.com/blog/post/explained-the-wormhole-hack-february-2022)) | bridges are external multisigs bolted on after launch | light-client verification (`crates/btc-spv`, real Bitcoin headers) and rate caps (Master Prompt 25) | Bitcoin SPV verified in tests; no bridge |
| 4 | **Smart-contract arithmetic bugs** | Cetus (Sui), 22 May 2025, ~$223M: a silent overflow in a shared math library ([Cyfrin](https://www.cyfrin.io/blog/inside-the-223m-cetus-exploit-root-cause-and-impact-analysis), [Halborn](https://www.halborn.com/blog/post/explained-the-cetus-hack-may-2025)); resource types did not prevent it | libraries are copied into every protocol; the VM does not know what an invariant is | chain-enforced invariants and outflow rate limits (Master Prompt 23); the invariant guard (invariant 28) | the guard is REAL for native modules; contract-level invariants not built |
| 5 | **Quantum exposure** | ~6.26M BTC with exposed public keys (Chaincode Labs 2025, via [Onramp](https://onrampbitcoin.com/knowledge-center/which-bitcoin-is-vulnerable-to-quantum-computing-address-types-exposure-tiers-and-what-you-can-do); [Chaincode paper](https://chaincode.com/bitcoin-post-quantum.pdf)) | changing the signature scheme needs a soft fork and every holder to move coins | hybrid ML-DSA + SLH-DSA from genesis (TX-1) | REAL |
| 6 | **Finality latency** | Ethereum ~15 min to finality; the roadmap targets seconds by 2029 ([ethereum.org SSF](https://ethereum.org/roadmap/single-slot-finality/), [CoinDesk, 26 Feb 2026](https://www.coindesk.com/tech/2026/02/26/ethereum-foundation-drops-most-ambitious-roadmap-in-years-targets-finality-in-seconds-by-2029)) | a very large validator set, and changes gated by safety | DAG-BFT (ADR-015), target p50 ≤ 2 s | engine simulated, **not wired**; the node runs PoW |
| 7 | **Liveness** | Solana halts: ~18 h Sep 2022, ~19 h Feb 2023, ~5 h Feb 2024 ([Helius](https://www.helius.dev/blog/solana-outages-complete-history)) | high-throughput design, restarts coordinated off-chain | halt-before-fork upgrades, a rehearsed coordinated restart (`reports/19-operations.md`) | rehearsed on 12 local nodes |
| 8 | **Node cost** | Ethereum full node: >3 TB by mid-2025; 8-core, 32–64 GB RAM recommended ([Cherry Servers](https://www.cherryservers.com/blog/ethereum-node-requirements), [geth docs](https://geth.ethereum.org/docs/getting-started/hardware-requirements)) | a decade of state and history | pruning, the MMR history accumulator (`crates/history-compactor`), state sync | pruning REAL; **PQ transactions are 13 KB, which makes this worse, not better** (`reports/13-pq-weight.md`) |
| 9 | **Account usability** | Ethereum added EIP-7702 to *upgrade* EOAs on 7 May 2025 ([EF blog](https://blog.ethereum.org/2025/04/23/pectra-mainnet)) | billions in EOAs that cannot be removed | native smart accounts as the only account type (Master Prompt 22) | not the default: hybrid key accounts are |
| 10 | **MEV** | sandwiches ~$60M/yr in trader losses, falling as flow moves private ([Cointelegraph × EigenPhi](https://cointelegraph.com/research/exclusive-data-from-eigenphi-reveals-that-sandwich-attacks-on-ethereum-have-waned)) | an open mempool and continuous AMMs | uniform-price batch settlement (ADR-025); a sandwich simulated at +144M → −1 | REAL for the native DEX |
| 11 | **Existing post-quantum chains** | QRL: XMSS since June 2018, adding ML-DSA in its Zond upgrade ([QRL docs](https://docs.theqrl.org/what-is-qrl/)) | XMSS is stateful; a migration is under way | stateless ML-DSA + SLH-DSA hybrid | **prior art: Maya2C is not the first post-quantum chain** |

Row 11 is why "first post-quantum L1" must never be claimed
(`docs/prior-art/pq-from-genesis.md`).
