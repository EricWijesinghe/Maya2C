# Prior art: Randomness and VRF verification for smart contracts

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: "on-chain randomness API for smart contracts Aptos randomness Algorand VRF opcode Chainlink VRF Sui random"; "Algorand vrf_verify opcode AVM; Solana randomness Switchboard; NEAR random_seed host function; Internet Computer raw_rand" — the IC `raw_rand` API did not surface and is marked thin).

Feature: parity row P-40, [ADR-127](../adr/ADR-127-vrf-host-function.md); beacon findings in ADR-115 and [post-quantum-vrf.md](post-quantum-vrf.md) (branch `docs/adr-110-115-gaps`).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Algorand AVM 7 | Opcodes `block` and `vrf_verify`: a contract verifies a VRF proof on chain against a key, which is how its randomness beacon and oracles are built. | [developer.algorand.org](https://developer.algorand.org/articles/randomness-on-algorand), [algorand.co blog](https://algorand.co/blog/randomness-has-arrived) |
| Aptos | Native randomness from the validator set (Pinkas WVUF with DKG), synchronous in Move; randomness-reading functions must be private entry functions so a wrapper cannot revert on a losing draw. | [Aptos randomness deep dive](https://aptos-intelligence.vercel.app/reports/randomness-deep-dive), [0xsoftboi landscape](https://0xsoftboi.github.io/blog/the-on-chain-randomness-landscape/) |
| Sui | Reads the external drand beacon; same private-entry restriction. | [0xsoftboi landscape](https://0xsoftboi.github.io/blog/the-on-chain-randomness-landscape/) |
| Chainlink VRF v2.5 | Request/fulfil oracle service with an on-chain proof check; a 2024 paper counts 10.5 M requests in 2022. | [chain.link/vrf](https://chain.link/vrf), [ePrint 2024/957](https://eprint.iacr.org/2024/957.pdf) |
| NEAR `random_seed` | 32 bytes derived from the block producer's signature, previous epoch's value, height and timestamp. | [NEAR tutorial (docs preview)](https://near-docs-pr-2756.onrender.com/tutorials/coin-flip/introduction) |
| Solana (Switchboard) | Oracle VRF modelled on Algorand's. | [Wikipedia mirror](https://iiab.live/kiwix/content/wikipedia_en_all_maxi_2023-05/A/Verifiable_random_function) |
| Internet Computer | Threshold-BLS random beacon; `raw_rand` API not found this round. | [weekinicp](https://weekinicp.substack.com/p/decentralized-and-verifiable-randomness) (thin) |

## What Maya2C has (checked in repo, 2026-11-12)

- `block_randomness(out_ptr)` host function returns the previous block's beacon or `-1` without an oracle (`crates/vm/src/runtime.rs:635`, `:853-873`; `crates/node/src/state/contracts.rs:180`).
- Beacon: ECVRF (RFC 9381, `crates/vrf`) folded by hash, miner has one bit of influence per block (`crates/node/src/oracle/beacon.rs:5-9`, `:27`); ADR-115 adds a BLAKE3 hash-chain scheme.
- No way for a contract to verify a VRF proof of its own; no nested-call restriction on randomness reads.

## How the Maya2C version differs

- `vrf_verify` follows Algorand's opcode shape with a scheme tag from `crates/vrf` (ECVRF now, hash-chain under ADR-115, lattice refused until its gate) and measured fuel charged first (invariant 21's pattern).
- The future-value pattern is a composition of contract timers (ADR-114) and the beacon; no new randomness source and no external committee (Sui/drand, Chainlink).
- The Sui/Aptos private-entry rule is enforced by the host (`-4` on a nested call) rather than by a compiler.

**Verdict: similar exists (Algorand `vrf_verify`, Aptos/Sui restrictions, Chainlink VRF). Differentiate by the scheme-tagged verifier over the chain's own VRF crate and the host-enforced nesting rule. No "first" claim.**
