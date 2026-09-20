# Workspace map

What each crate is for, and -- where it matters -- *why it is a separate
crate*. Most of those reasons are boundaries somebody has to keep: a crate
that must stay dependency-free so Kani can compile it, a feature that must
stay default-off so a GPU-less runner can build the workspace, a crate that
must not be a workspace member because it targets a different architecture.

`cargo metadata --no-deps` is the authority for membership; this file is the
authority for intent. Machine-readable status is `features.toml`.

| Member | Role |
|---|---|
| *(root)* `custom-l1-node` | Node daemon: consensus, chain, p2p, RPC, metrics |
| `ledger-math` | All `u64` credit/debit/nonce math. Kani-verifiable — keep RocksDB out |
| `crypto-pq` | SLH-DSA instantiation. Must stay the monomorphizing crate |
| `zk-privacy` | Groth16 shielded joinsplits |
| `vm` | Wasm contract execution |
| `l2-flash` | L2 settlement |
| `wallet`, `wallet-gui/core` | CLI + GUI wallet |
| `explorer` | Chain explorer |
| `cuda-miner` | GPU miner. `cuda` feature OFF by default so CI stays green without a CUDA toolkit |
| `stratum-v2` | Pool protocol, no chain dependency — fuzzable in isolation |
| `pool-service` | Daemon joining SV2 to chain types (PPLNS, payouts) |
| `dex` | Constant-product curve + order book + batch clearing. Kani-verifiable — keep it dependency-free |
| `vrf` | RFC 9381 EC-VRF behind the randomness beacon. Pinned to the RFC's test vectors |
| `governance` | Proposal lifecycle, vote tally, and the bounds a proposal may never escape. Kani-verifiable — dependency-free |
| `fee-market` | EIP-1559 base fee over serialized **bytes** (there is no block gas), 80/20 burn/treasury split, supply cap. Research branch: `FeeConfig::DISABLED` (activation `u64::MAX`), called by nothing in `src/` — `tests/fee_market_tests.rs` checks both. Dependency-free for Kani |
| `faucet` | Testnet faucet. Funded key behind a public endpoint; the per-IP/per-address limiter and daily cap are the whole of what bounds a drain |
| `telemetry` | Network telemetry. `server`/`client` feature split so a miner links no web framework; the always-on half builds for `wasm32` |
| `custody-mpc` | Threshold custody of a chain key: dealerless Pedersen VSS, ML-KEM-sealed shares, quorum signing. Links no chain types — `tests/custody_parity_tests.rs` pins its derivation to `crypto::hybrid` |
| `zkml` | Verifies halo2 (KZG/BN254) proofs of quantized-classifier inference for `host_verify_zkml_proof`. Hand-written circuit, verifier only. **Dark** (`ZKML_ACTIVATION_HEIGHT = u64::MAX`), not post-quantum, SRS from a public seed |
| `zkml-prover` | Off-chain half of `zkml`: ONNX import via `tract-onnx`, key generation, proving. Its own crate, not a feature, so the node's graph cannot reach tract; `rust-version = 1.91` because tract's patched releases need it, and nothing the node links depends on it |
| `iso20022` | ISO 20022 bank-rail messages and the bridge to a payment intent. Chain-free, so its XML decoder fuzzes alone (`fuzz/fuzz_targets/iso20022_decode.rs`). One compiled-in amount scale; an inexact amount is an error, never rounded |
| `archive` | CAR v1 (zstd) archives of pruned block batches and the stores that hold them: local dir, kubo IPFS, Arweave gateway (read-only). Chain-agnostic, so the decoder of untrusted archives is fuzzable alone (`fuzz/fuzz_targets/car_decode.rs`). See `docs/pruning.md` |
| `htlc-lattice` | Lattice HTLCs: Module-LWE commitment `t = A·s + e` at ML-DSA-65's parameters, the short-opening check, the timelock rule (Kani-proved exclusive), the lock record. Chain-free, decoders fuzzable alone (`fuzz/fuzz_targets/htlc_lattice_decode.rs`). No RNG: entropy is the caller's — [docs/htlc-lattice.md](docs/htlc-lattice.md) |
| `htlc-watcher` | Counterparty watcher: claims on revelation, refunds on expiry, refuses unsafe pairings and late reveals. Pure `policy::decide`, `SwapChain` trait over RPC. Its own process because it holds a hot key |
| `neural-gas-trainer` | Off-chain half of the neural base-fee gain: synthetic demand simulator, float SGD, quantization into `fee-market/src/model/weights_v1.rs` (`-- --check` fails on drift). Floats stop here — the node does not depend on it (invariant 20). The integer network and envelope live in `fee-market`; block features in `src/neural_gas/` — [docs/neural-gas.md](docs/neural-gas.md) |
| `stateless-core` | Keyed sparse Merkle tree over accounts, its canonical witness, transfer execution against a verified witness, and a Ring-SIS research backend over `Z_q[x]/(x^256+1)`. Chain-free, so a stateless verifier cannot reach a database and the witness decoder fuzzes alone (`fuzz/fuzz_targets/stateless_witness_decode.rs`). The node's sparse accounts root is computed *by* this crate — [docs/stateless.md](docs/stateless.md) |
| `ebpf-net/common` | What the XDP program and user space share byte for byte: the 56-byte relay header, the integer token bucket, the verdict order, the map records. `no_std`, dependency-free, compiled for the host *and* `bpfel-unknown-none` — the host tests exercise the function the kernel runs |
| `ebpf-net` | Block relay over UDP beside gossip: XChaCha20-Poly1305 chunks under per-peer keys, bounded per-sender reassembly, the receiver. Under `src/linux/` behind the `xdp` feature (off by default): aya loader, hand-rolled AF_XDP over `libc`, throughput harness. Chain-free, decoder fuzzable alone (`fuzz/fuzz_targets/xdp_relay_decode.rs`) — [docs/ebpf-net.md](docs/ebpf-net.md) |
| `offsec-sandbox/` | Autonomous red-team fuzzing. **Not a workspace member** — its own `[workspace]` and lockfile, like `fuzz/`, so LibAFL (optional) and Z3 (optional, arithmetic crates only) never touch the node graph or Kani. Structure-aware mutators for tx/WASM/handshake, a panic-and-determinism oracle over the real apply path, and crash triage that emits regression stubs, never patches (invariant 13). The in-tree gate is `tests/fuzz_harness.rs`; the two share no dependency — [docs/offsec-sandbox.md](docs/offsec-sandbox.md) |
| `threat-intel` | Threat indicators from evidence a third party can re-check: an ed25519 gossip author's own signature over a frame that decodes and fails a stateless check. Rebuilds libp2p's signed bytes itself; integer score halving by height; mitigation computed, never stored. Dependency-free for Kani — verification lives in `src/state/threat_exec.rs`; decoder fuzzes alone (`fuzz/fuzz_targets/threat_evidence_decode.rs`) — [docs/threat-intel.md](docs/threat-intel.md) |
| `threat-firewall` | Per-host worker: joins a co-located node's indicators (by author) with its own connection addresses and drives nftables / iptables. Pushes rules nowhere else, holds no chain key, never blocks loopback |
| `iot-anchor` | Hardware-anchored sensor identity: ML-DSA-65 device keys, PUF fuzzy extractor, TPM seed sealing (`tpm` feature, `tpm2-tools`, off by default), Merkle-rooted telemetry batches, tamper and clone evidence, and the Kani-proved batch rules. `no_std` and heap-free — builds for `thumbv8m.main-none-eabihf` and `riscv32imc-unknown-none-elf`; decoders fuzz alone (`fuzz/fuzz_targets/iot_anchor_decode.rs`) — [docs/iot-anchor.md](docs/iot-anchor.md) |
| `iot-firmware/` | Example Cortex-M33 firmware. **Not a workspace member** — its own `[workspace]`, `thumbv8m.main-none-eabihf`. Runs the device lifecycle under QEMU `mps2-an505` (WSL) and reports the stack high-water mark: 258,532 bytes. Deterministic RNG and simulated PUF — a code path, not security |
| `ebpf-net/programs` | The XDP program. **Not a workspace member** — nightly, `bpfel-unknown-none`, `-Z build-std=core`. Needs the **prebuilt** `bpf-linker` v0.11.1 (`cargo install` fails without a system libLLVM matching rustc's); builds on Windows and Linux. Loading needs Linux: the `Ubuntu-24.04` WSL distro on this machine runs `ebpf-net/tests/xdp_veth.rs` as root |
| `dashboard/` | Leptos browser page. **Not a workspace member** — CSR Leptos only runs on `wasm32`. Built with `trunk` |
