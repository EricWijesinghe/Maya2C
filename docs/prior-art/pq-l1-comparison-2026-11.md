# Prior art: post-quantum L1 comparison, November 2026

**Search status:** searched and accessed 2026-11-01. Web search plus page fetches
of primary pages where reachable. Extends [pq-from-genesis.md](pq-from-genesis.md)
(re-search 2026-10-28). This file authorizes **no** "first", "only" or
"unprecedented" claim; see the last section.

**Search boundary.** Chains: QRL, Algorand, Ethereum, Bitcoin, Solana, Cardano,
Aptos, Sui, IOTA, Mochimo, Abelian, Cellframe, plus launches found in 2026
(Quantus, Naoris, QoreChain, NEAR, Verona). Not searched: other Cosmos-chain PQ
forks, Hedera, Polkadot/Substrate, Tezos, Zcash, BTQ/Bitcoin Quantum (one
search returned nothing). Several sites returned 403 (globenewswire, the
Mochimo Medium blog); those rows rest on a secondary page. "Finality" cells
marked *(background)* come from general knowledge of the chain, were **not**
fetched this session, and must be sourced before publication.

**Weights.** P = primary (project docs, BIP/EIP text, official blog). S =
secondary or press release. A = aggregator or index (QRI, layerqu), draft
quality. Anything resting on S/A alone is called out.

## Maya2C side, verified against the tree

| Claim | Tree evidence | Verdict |
|---|---|---|
| Hybrid ML-DSA-65 + SLH-DSA-SHA2-128s, both must verify, default account suite | ADR-051 context table; `features.toml` `hybrid-signature-transaction-validation` = verified, REAL | Keep |
| P2P transport ML-KEM-1024, ML-KEM-768 fallback | `docs/pq-transport.md`; `features.toml` `p2p-mlkem-1024` = working, REAL ("not externally reviewed"; `Required` policy not wired to operator config). Layered **above** libp2p Noise (X25519), not instead of it | Keep, worded as "ML-KEM layered on Noise" |
| DAG-BFT finality | `features.toml` `dag-bft` = verified, REAL; wired by ADR-027 | Keep. No finalized-TPS number exists (CLAUDE.md) |
| GPU mining never orders or finalizes blocks | ADR-044; `mining-rewards-lane` = working, REAL, but "off on a chain whose genesis has no bft.mining; **not yet on the testnet**". ADR-068 (forever mining) "takes effect at the next re-genesis" | Keep only as "design, enabled by genesis config"; not live |
| 256-bit amounts | `amount-256` = working; SHIPPED in architecture-vision | Keep |
| 24 decimals | `crates/amount/src/lib.rs:52` `COIN_DECIMALS = 24` on origin/master (#272, ADR-068); `apps/wallet-gui/core/src/amount.rs` reads it. Takes effect on mainnet genesis; the stopped testnet was 18 | Kept (not a differentiator on its own) |
| Batch-auction DEX in base layer | `crates/dex/src/batch.rs` `clear_batch`, referenced from `crates/node/src/state/dex_exec.rs`. The `features.toml` entry is "Constant-product DEX + order book", not batch | Keep as "uniform-price batch clearing per block"; activation height not checked |
| PQ chat app | `maya-chat` = working but class **RESEARCH**, "unaudited; no groups, no metadata privacy" | Not a chain comparison point; see [p2p-chat.md](p2p-chat.md). Dropped here |

Maya2C status: testnet. No mainnet launch is recorded in the files read;
**unconfirmed** whether one has happened since.

## Comparison table (accessed 2026-11-01)

| Chain | Sig scheme | Mandatory? | Dual-sig? | PQ transport? | Finality | Status (date) | Source (weight) |
|---|---|---|---|---|---|---|---|
| **Maya2C** | ML-DSA-65 + SLH-DSA-SHA2-128s; ML-DSA-87 opt-in suite | Default account suite; other suites exist (ADR-007/013) | Yes, two PQ families, both required | Yes, ML-KEM-1024/768 over Noise | DAG-BFT | Testnet | In-repo (above) |
| QRL 1.x | XMSS | Yes (spending is XMSS only) | No | Not documented | PoW | Mainnet since 2018 (see pq-from-genesis.md) | [QRI, 2026-08-20](https://qrindex.org/projects/quantum-resistant-ledger/) (A) |
| QRL 2.0 (Zond) | ML-DSA-87 | Unconfirmed | Unconfirmed | Unconfirmed | PoS; mechanism unconfirmed | Testnet V2 Q1 2026; mainnet gated on audits, "50% complete" (2026-06-26) | [QRL blog](https://www.theqrl.org/blog/celebrating-8-years.md) (P). SPHINCS+ "post-mainnet" is from an aggregator, unconfirmed |
| Algorand | Falcon: state proofs; Falcon-1024 native accounts; Falcon-512 planned | Opt-in accounts | Hybrid Ed25519+Falcon accounts and consensus planned, end 2026 | Not addressed | Not addressed on page *(background: Pure PoS)* | Native PQ accounts "live on Mainnet as of August 2026" (page footnote, no day); roadmap post dated 2026-06-18. Falcon LogicSig tx on mainnet 2025-11-03 (pq-from-genesis.md) | [Algorand roadmap](https://algorand.co/blog/algorand-post-quantum-cryptography-roadmap) (P); [state proofs](https://dev.algorand.co/concepts/protocol/state-proofs/) (P, no date) |
| Ethereum | leanXMSS (consensus, planned); Falcon/ML-DSA/SPHINCS+ "being evaluated" for execution | Not decided | Not decided | None stated | Casper FFG *(background)* | Proposal. No milestone live. Team estimate 2029. EIP-8355 (ML-DSA precompiles 0x12-0x14) Draft, created 2026-07-30, no fork scheduled | [pq.ethereum.org](https://pq.ethereum.org/) (P); [EIP-8355](https://eips.ethereum.org/EIPS/eip-8355) (P) |
| Bitcoin | None specified. BIP-360 (P2MR) removes the key-path spend; spec text: "does not include the introduction of post-quantum signature schemes" | n/a | n/a | n/a | PoW *(background)* | BIP-360 Draft, merged to BIP repo Feb 2026 (10 or 11 Feb, sources disagree); not activated | [BIP-360 text](https://github.com/bitcoin/bips/blob/master/bip-0360.mediawiki) (P). BIP-361 unverified (fetch 404) |
| Solana | Winternitz one-time vault (WOTS, truncated Keccak256) | Opt-in; "not a default feature" | No | None found | Not checked *(background: Tower BFT)* | Vault introduced via GitHub 2025-01-03; no audit mentioned. Mainnet deployment **unconfirmed**. Falcon test with Project Eleven reported 2026-04 (S): ~90% throughput drop in a test environment | [Quantum Insider, 2025-01-04](https://thequantuminsider.com/?p=2373792) (S) |
| Cardano | Ed25519 stays; CIP-0197 adds ZK seed proofs | Opt-in, proposal | No | None found | Not checked *(background: Ouroboros)* | CIP-0197 in community review since 2026-08-23; "not yet implemented" | [Coingape, 2026-08-24](https://coingape.com/cardano-opens-review-on-quantum-proof-wallet-cip-as-upgrade-2026-gains-momentum/) (S). CIP text not fetched |
| Aptos | SLH-DSA-SHA2-128s (AIP-137) | Opt-in | No | None found | Not checked | Proposal, awaiting acceptance; page says "no urgency to deploy"; no date on page | [AIP-137](https://gregnazario.github.io/AIPs/aips/137/) (P, mirror) |
| Sui | ML-DSA-65 accounts; SLH-DSA-SHA2-128s in Move vaults | Opt-in ("additive") | Optional: multisig Ed25519 + ML-DSA-65 | None found | Not checked *(background: Mysticeti)* | Blog 2026-08-06: nothing live; vaults mainnet "this year", accounts testnet end-2026, mainnet Q1 2027. Devnet v1.82.0 ML-DSA-65 on 2026-10-12 (S) | [Sui blog](https://www.sui.io/blog/making-sui-quantum-ready) (P); devnet date via [cryptobriefing](https://cryptobriefing.com/sui-first-quantum-safe-devnet-transaction/) (S, search snippet only) |
| IOTA | Base ledger: no PQ scheme found. Identity library v1.7 (beta): ML-DSA, SLH-DSA, Falcon, ML-DSA+Ed25519 composite | Opt-in, identity layer only | Composite JWK (identity only) | None found | Not checked | Base-ledger PQ: not found. WOTS dropped 2020-21 per U.Today (S) | [IOTA docs](https://docs.iota.org/developer/iota-identity/how-tos/post-quantum) (P); [U.Today](https://u.today/iota-implements-post-quantum-upgrade-dominik-schiener-teases-exciting-times-to-come) (S) |
| Mochimo | WOTS+ | Yes, since genesis, no classical path | No | Not documented (plaintext TCP per one S) | PoW, no finality layer | Mainnet since 2018-06-25 (aggregator); QRI 87/100 on 2026-08-20; no usable independent audit of current code | [QRI](https://qrindex.org/projects/mochimo/) (A). PoS v4.0 testnet: unconfirmed |
| Abelian | Lattice (Dilithium/Kyber family) with linkable ring sigs | Unconfirmed | Unconfirmed | Unconfirmed | PoW | Mainnet "since April 2022" (self-reported, event page); "Blanc Fork" crypto-agility upcoming, no date | [Luma PQBD 2026](https://luma.com/47tylonu) (S, self-reported, search snippet only). Other pages A |
| Cellframe | Dilithium, Falcon, SPHINCS+ (sources conflict on SPHINCS+), Shipovnik; Kyber-512 KEM | Unconfirmed | Dilithium "can run in hybrid mode" (docs) | Kyber-512 listed; whether it protects node transport: unconfirmed | Unconfirmed | Mainnet release announced 2022 (S); current status unconfirmed | [Cellframe wiki](https://wiki.cellframe.net/02.+Learn/Technology/Architecture/Encryption+Protocols) (P; does not say what is deployed) |
| NEAR | ML-DSA (FIPS 204), account-level | Opt-in key rotation | No | Not covered | Not covered | Mainnet upgrade 2.13, 2026-07-20 | [PR Newswire](https://www.prnewswire.com/news-releases/near-protocol-brings-quantum-safe-signing-to-mainnet-302829646.html) (S, vendor) |
| Verona | ML-DSA-65 native account authenticator | Opt-in; optional "PQ-required" policy can force classical + PQ co-signing | Optional, per account | Not covered | CometBFT; validator keys to ML-DSA-65 in phase 2, undated | Fermi v32 mainnet, ~2026-10-28 (inferred from an image filename) | [Yellow](https://yellow.com/news/verona-launches-fermi-mainnet-v32-with-nist-post-quantum-cryptography) (S) |
| Quantus | ML-DSA-65 and ML-DSA-87 | Unconfirmed | Unconfirmed | Unconfirmed | PoW, 12 s target; no finality layer stated | Mainnet 2026-09-10; four firms reviewed pre-launch, per the article | [KuCoin flash](https://www.kucoin.com/news/flash/quantus-launches-mainnet-with-post-quantum-cryptography-from-block-one) (S) |
| Naoris | Not named ("NIST-approved") | Unconfirmed | Unconfirmed | Unconfirmed | dPoSec, no finality detail | Mainnet 2026-04-01, invite-only phases; no audit mentioned | [Quantum Insider](https://thequantuminsider.com/2026/04/01/naoris-protocol-launches-mainnet-introducing-post-quantum-layer-1-blockchain/) (S) |
| QoreChain | ML-DSA-87 sigs, ML-KEM-1024 key encapsulation; one article says hybrid secp256k1 + ML-DSA-87, another omits it | Unconfirmed | **Conflicting** | KEM listed, transport use not stated | PRISM; throughput/finality figures are design goals, not measured | Mainnet announced for 2026-06-07; PQ transaction 2026-07-02; no audit mentioned | [QCR, 2026-07-03](https://quantumcomputingreport.com/qorechain-implements-complete-nist-post-quantum-cryptography-stack-on-public-mainnet/) (S, company claims) |

## Where Maya2C differs, stated precisely

Only differences the table supports. Each means "not found in this search",
not "nobody does it".

1. **Two PQ signature families, both required, on every default-suite
   transaction.** Among the 18 other rows, none was found requiring two
   post-quantum families. Nearest: Sui (optional Ed25519 + ML-DSA multisig),
   Verona (optional classical + PQ co-sign policy), Algorand (ECC + Falcon
   hybrid planned), QoreChain (secp256k1 + ML-DSA-87, conflicting), Cellframe
   (Dilithium hybrid mode, per docs). Those pair a classical scheme with a PQ
   one; that is a different design, not a better or worse one.
2. **Stateless PQ signatures only in the default path.** ML-DSA and SLH-DSA
   need no one-time-key state. QRL 1.x (XMSS), Mochimo (WOTS+) and the Solana
   vault (WOTS) use stateful or one-time hash schemes.
3. **A PQ KEM layer on peer transport, documented, with a version-negotiated
   fallback.** Where transport was addressed at all (QRL, Mochimo: not
   documented), none documented a PQ layer. NEAR, Algorand, Sui, Aptos and
   Verona: not covered. Cellframe (Kyber-512 listed) and QoreChain
   (ML-KEM-1024 listed) are open: deployment unconfirmed. Caveat: it sits above
   classical Noise, and `p2p-mlkem-1024` is "working", not externally reviewed.
4. **BFT finality with ML-DSA-65 validator votes**, on a running testnet.
   QRL 1.x, Mochimo and Quantus are PoW with no stated finality layer;
   Verona PQ validator keys are phase 2, undated.
5. **Mining that does not order blocks** (ADR-044/068). Quantus and Mochimo
   mine to produce blocks; the Maya2C lane pays shares from a genesis reserve
   while BFT orders. Status: genesis-configured, **not on the testnet yet**.
   Describe as design, not as deployed.

Wording that is safe: "every default-suite transaction carries ML-DSA-65 and
SLH-DSA-SHA2-128s signatures, and both must verify." Not safe: "first",
"only", "the most secure", "quantum-proof".

## Where others are ahead

- **Time in production.** QRL 1.x (2018) and Mochimo (2018-06-25 per an
  aggregator) have run PQ-only spending on mainnet for years. Abelian reports
  mainnet since April 2022 (self-reported). Maya2C has a testnet.
- **Mainnet PQ now.** Algorand (Falcon accounts, August 2026; Falcon state
  proofs), NEAR (2026-07-20), Verona (~2026-10-28), Quantus (2026-09-10),
  Naoris (2026-04-01) and QoreChain (announced 2026-06-07) all report
  PQ-signature mainnets. Maya2C does not.
- **Audits.** Quantus reports four firms plus a public competition before
  launch; QRL 2.0 reports 50% of audits complete (June 2026). Maya2C
  `p2p-mlkem-1024` and `maya-chat` carry "not externally reviewed / unaudited"
  in `features.toml`. Mochimo has no usable independent audit of current code
  (QRI), so audits are not a clean win for anyone.
- **Installed base.** NEAR, Sui, Aptos, Solana, Cardano and Ethereum have
  large user bases; their PQ work is opt-in migration on top of that.
- **Smaller signatures.** Falcon (Algorand; evaluated by Ethereum) gives much
  smaller signatures than ML-DSA-65 (3,309 B) plus SLH-DSA-SHA2-128s
  (7,856 B). The Maya2C dual-signature byte cost is real; the TPS cost is
  MISSING (ADR-051).
- **Standards track.** Ethereum has a dedicated team, a public roadmap and an
  EIP draft (8355). Bitcoin BIP-360 is in the official repository.

## Claims dropped or downgraded

- "24 decimals": corrected after review. The first draft read a stale checkout; origin/master has 24 (#272). Kept as a fact, not claimed as a differentiator.
- "GPU mining live": not on the testnet. Downgraded to design.
- "PQ chat app" as a chain differentiator: class RESEARCH, unaudited; Signal,
  iMessage and SimpleX already ship PQ E2EE (p2p-chat.md). Dropped.
- "Batch-auction DEX" as distinctive: no DEX comparison was searched. Not
  claimed.
- "First PQ L1" and similar: forbidden; QRL dates from 2018.
- Unconfirmed in this search: Cellframe current status, Abelian current status
  and signature details, QoreChain hybrid signing, Verona exact date, Solana
  vault mainnet deployment, BIP-361, Naoris scheme, QRL 2.0 finality, Maya2C
  mainnet status, and every *(background)* finality cell.
