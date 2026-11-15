# Prior art: Post-quantum verifiable random functions

**Search status:** searched 2026-11-12 (WebSearch: "post-quantum verifiable random function lattice LaV X-VRF 2025 standardization"; WebFetch of ePrint 2020/1222 listing, 2021/302, 2022/141, 2025/766). DeuringVRF was **not fetched** this round; it is cited from the CATALOG `pq-randomness` row. No NIST, IETF CFRG or ISO document on a post-quantum VRF surfaced in the search; that is a statement about this search, not about the world.

Feature: parity row P-32, CATALOG row `pq-randomness`, [ADR-115](../adr/ADR-115-post-quantum-vrf.md).

## What exists

| Construction | Assumption | Shape | Source |
|---|---|---|---|
| LB-VRF, Esgin et al. (FC 2021) | Module-SIS, Module-LWE | Few-time: a key supports a bounded number of evaluations; proposed for Algorand-style sortition. | [ePrint 2020/1222](https://eprint.iacr.org/2020/1222), [Monash listing](https://research.monash.edu/en/publications/practical-post-quantum-few-time-verifiable-random-function-with-a/) |
| X-VRF, Buser et al. (ESORICS 2022) | Hash functions (XMSS) | Many-time but stateful; the blockchain's state manages the XMSS index to keep outputs unique. | [ePrint 2021/302](https://eprint.iacr.org/2021/302) |
| LaV, Esgin, Steinfeld, Liu, Ruj (CRYPTO 2023) | Module-SIS, Module-LWE | Long-term lattice VRF; the abstract claims the shortest outputs among standard-assumption post-quantum proposals; no proof-size figure on the listing page. | [ePrint 2022/141](https://eprint.iacr.org/2022/141), [Esgin slides, EPFL 2023](https://lattices-meet-hashes23.epfl.ch/slides/Esgin.pdf) |
| DeuringVRF, Leroux (Eurocrypt 2025) | Isogenies | Long-term; not fetched this round. | CATALOG `pq-randomness` row |
| Brandt (2025) | Generic assumptions | A transform to strong unbiasability (Giunta–Stewart notions) in the standard model without setup; plausibly post-quantum. | [ePrint 2025/766](https://eprint.iacr.org/2025/766) |
| RFC 9381 ECVRF | ECDLP | What `crates/vrf` implements; the only VRF standard found. | [RFC 9381](https://www.rfc-editor.org/rfc/rfc9381) |
| Algorand | — | Lists post-quantum VRF as research toward 2027 (parity matrix, innovations 12). | [Algorand quantum resilience post](https://algorand.co/blog/algorand-targets-broad-quantum-resilience-by-2027) |

Why FIPS 204/205 signatures are not VRFs: a signer can produce more than one valid signature for the same message and a verifier cannot tell, so hashing a signature gives the signer a choice of outputs. Deterministic signing constrains the signer's algorithm, not what the verifier can check.

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/vrf`: ECVRF-EDWARDS25519-SHA512-TAI, classical, stated at the top (`lib.rs:1-26`); scheme tag byte on every key (`scheme.rs:1-18`, `:39-89`); suite octet `0x03` pinned (invariant 10).
- Consumer: the optional oracle beacon (`crates/node/src/oracle/beacon.rs:5-9`, `registry.rs:161`), one bit of miner influence stated (`beacon.rs:23-35`).
- Committee selection does not use a VRF: ranked stake (`crates/staking/src/types.rs:175-187`).

## How the Maya2C version differs

- **Honest shape first**: scheme 2 is a hash-chain commit-reveal built from BLAKE3 alone, documented as not a VRF in the RFC sense, with its withholding bias stated and bounded by the accumulator; X-VRF's "chain-managed state" idea is used for the chain index.
- **A mixed beacon** that folds ECVRF and scheme 2, so the output is unpredictable while either stands.
- **A written research gate** (peer review plus independent cryptanalysis, two implementations with vectors, measured size and timing, constant-time evidence) before a lattice scheme is compiled; the tag is reserved and refused until then, the way an unregistered suite byte is refused (invariant 29).

**Verdict: constructions exist (LB-VRF, X-VRF, LaV, DeuringVRF); no standard exists. Maya2C ships a hash-only scheme with its limits written down and gates the lattice one. No "first" claim.**
