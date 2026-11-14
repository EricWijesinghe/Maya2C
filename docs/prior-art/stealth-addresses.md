# Prior art: Stealth addresses over ML-KEM (one-time payment addresses)

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: gap row C4 ([docs/next-evolution-gap.md](../next-evolution-gap.md)), [ADR-102](../adr/ADR-102-stealth-addresses-ml-kem.md). Not built.

**Queries:** "post-quantum stealth address protocol ML-KEM lattice paper 2024 2025"; "ERC-5564 stealth addresses view tag Umbra Monero CryptoNote one-time address"; "Monero CryptoNote one-time address view key scanning; Bitcoin BIP-352 silent payments".

## Who else does something similar

| Project or paper | What it does | Source |
|---|---|---|
| Monero (CryptoNote) | Every payment goes to a one-time output key derived from the recipient's public view and spend keys plus a sender ephemeral key. The private view key detects incoming outputs without the spend key. Elliptic-curve key homomorphism makes the spend unlinkable to the published address. Not post-quantum. | [Bitcoin Core dev transcript, Monero note](https://registry-hpzwcyjgm-btc-knowledge-base.vercel.app/bitcoin-core-dev-tech/2024-04/silent-payment-descriptors) |
| ERC-5564 / ERC-6538 (Ethereum) | Stealth meta-address registry and an announcement event with a one-byte **view tag** as the first metadata byte; the tag is a truncated hash of the shared secret so a scanner skips most announcements before the expensive step. The authors settled on one byte after trying 32, 4 and 2. secp256k1 only. | [ERC-5564](https://eips.ethereum.org/EIPS/eip-5564), [Magicians thread](https://ethereum-magicians.org/t/erc-5564-stealth-addresses/10614), [ERC-6538](https://ercs.ethereum.org/ERCS/erc-6538) |
| Umbra | Pre-dates ERC-5564; a different secp256k1 derivation; stealth payments on Ethereum and L2s. | [QuickNode guide](https://www.quicknode.com/guides/ethereum-development/wallets/how-to-use-stealth-addresses-on-ethereum-eip-5564) |
| BIP-352 Silent Payments (Bitcoin) | No announcement: the one-time taproot output is derived from the sender's inputs and the recipient's scan and spend keys. Separate scan key so scanning can be delegated. Receiver scanning is the main cost; light-client scanning is an open area. | [BIP-352](https://bips.dev/352/) |
| Mikić, Srbakoski, Praška, "Post-Quantum Stealth Address Protocols" (arXiv 2501.13733, Jan 2025) | Three lattice stealth address protocols (LWE, Ring-LWE, Module-LWE). The Module-LWE variant uses Kyber as the KEM and is reported as the fastest. Full unlinkability relies on lattice key homomorphism, i.e. a custom signature key structure, not FIPS 204 as published. Preprint; no peer-reviewed venue found. | [arXiv 2501.13733](https://arxiv.org/abs/2501.13733) |
| ERC-8441 (Ethereum, draft) | Hybrid: the payment secret combines an ML-KEM-768 encapsulation with a secp256k1 ECDH secret; announced as ERC-5564 scheme id 3. The post-quantum part protects the announcement layer only; spending stays on secp256k1. | [ERC-8441](https://eips.ethereum.org/EIPS/eip-8441) |
| SPECTER (GitHub project) | Post-quantum stealth payments on Ethereum and Sui keyed by ENS or SuiNS names, built on ML-KEM-768. Project status not verified. | [SPECTER](https://github.gg22.workers.dev/pranshurastogi/SPECTER) |

## What Maya2C has (checked in repo, 2026-11-12)

- `rg -i stealth crates docs features.toml`: no code, no ADR before ADR-102.
- ML-KEM-768 with the FIPS 203 sizes: `crates/crypto-pq/src/kem.rs:63-69` (`ENCAPSULATION_KEY_LEN` 1184, `CIPHERTEXT_LEN` 1088, `SHARED_SECRET_LEN` 32); `encapsulate` at `kem.rs:227`, implicit-rejection `decapsulate` at `kem.rs:253`.
- Sealed envelopes to a viewing key over ML-KEM: `crates/privacy/src/lib.rs:92-110` (`seal`, `open`), RESEARCH.
- Deterministic key from a seed: `crates/crypto-pq/src/suite/validator.rs:43` (`from_seed`, FIPS 204 `KeyGen_internal`).
- Addresses are a hash of suite byte and public key: `crates/crypto-pq/src/suite/mod.rs:313-318`.
- `sha3` is already a `crypto-pq` dependency (`crates/crypto-pq/Cargo.toml:89`, used in `kem_suite/dual.rs:20`).

## How the Maya2C version differs

- **KEM only, no key homomorphism.** FIPS 204 keys cannot be tweaked the way secp256k1 or the arXiv lattice schemes tweak them. ADR-102 therefore derives the one-time address as a hash commitment to the recipient's spend address and a KEM-derived tweak. Receipt is unlinkable; **the spend reveals the spend address**, which Monero and the arXiv schemes avoid. That limit is stated in the ADR, not hidden.
- **The view tag comes after decapsulation.** On secp256k1 the tag saves the second point multiplication. With a KEM there is nothing cheaper than decapsulation that depends on the shared secret, so the tag only saves the address derivation. Per-announcement scanning cost is one decapsulation; measured figures are MISSING until the implementation exists.
- **Scan key delegation holds**, as in BIP-352: the ML-KEM decapsulation key detects and derives, the signing key spends.
- **No hybrid with a classical curve** (contrast ERC-8441): Maya2C has no secp256k1 anywhere in consensus.

**Verdict: similar exists (Monero, ERC-5564, BIP-352; post-quantum variants in a preprint and an Ethereum draft). Differentiate by KEM-only derivation under FIPS 203/204 with the spend-time linkability stated. No "first" claim.**
