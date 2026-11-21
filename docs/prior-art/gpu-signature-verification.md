# Prior art: GPU batch verification of post-quantum signatures (advisory to a CPU verifier)

**Search status:** partially searched 2026-11-12 (web search; sources cited below). Not searched: WGSL or WebGPU implementations of ML-DSA or SLH-DSA specifically; other chains' internal use of GPU verification beyond Solana.

Feature: HW-3, [ADR-137](../adr/ADR-137-hardware-intelligence.md).

**Queries:** "NVIDIA cuPQC ML-DSA ML-KEM GPU batch library"; "GPU implementation Dilithium ML-DSA signature verification high-throughput paper ePrint"; "CUSPX GPU implementations SPHINCS+ paper"; "Solana validator GPU signature verification ed25519 solana-perf-libs still used".

## Who else does something similar

Every figure below is the source's own, on its own hardware and baseline.
None is a Maya2C measurement and none is carried into a Maya2C document.

| Project | What it does | Source |
|---|---|---|
| NVIDIA cuPQC (cuPQC-PK) | CUDA library with batched ML-KEM-512/768/1024 and ML-DSA-44/65/87. Vendor-published throughput is for ML-KEM on a data-centre GPU against one CPU core. Closed, one vendor. | [developer.nvidia.com/cupqc](https://developer.nvidia.com/cupqc), [docs](https://docs.nvidia.com/cuda/cupqc/overview/feature_cupqc_pk.html) |
| cuDilithium (Shen et al.) | CUDA implementation of Dilithium at all three levels, key generation, signing and verification; the authors report large speed-ups against a single CPU thread. | [ePrint 2024/1365](https://eprint.iacr.org/2024/1365.pdf), earlier [arXiv 2211.12265](https://arxiv.org/pdf/2211.12265) |
| cuML-DSA | Server-oriented CUDA design for ML-DSA **signing**. | [ePrint 2023/1522](https://ia.cr/2023/1522) |
| CUSPX (Wang, Dong et al.) | CUDA SPHINCS+ with three levels of parallelism; headline results are for signing. | IEEE Transactions on Computers 74(1), 2025, [doi 10.1109/TC.2024.3457736](https://unpaywall.org/10.1109%2FTC.2024.3457736) |
| Kim et al. | "Parallel Implementation of SPHINCS+ With GPUs", IEEE TCAS-I 2024. | [IEEE Xplore 10461494](https://ieeexplore.ieee.org/document/10461494) |
| GRASP (Ning and Dong) | CUDA SPHINCS+ throughput work. | [ePrint 2024/1030](https://eprint.iacr.org/2024/1030) |
| Römer et al. | "On GPU acceleration of PQC algorithms", CUDA, several schemes. | [ePrint 2025/1596](https://eprint.iacr.org/2025/1596) |
| Solana `solana-perf` | Ed25519 batch verification moves to a CUDA library when `perf-libs` is present and otherwise runs on all CPU cores. The GPU verdict is used directly. Whether current validators deploy it was not established by this search. | [docs.rs solana-perf sigverify](https://docs.rs/solana-perf/latest/solana_perf/sigverify/index.html) |

## What Maya2C has (checked in repo, 2026-11-12)

- CPU verification only, through `verify_at`, with a parallel prewarm into a
  successes-only cache: `crates/node/src/state/db.rs:1159-1195`,
  `crates/node/src/state/verified.rs`.
- Hybrid signature of 11,165 bytes, ML-DSA-65 plus SLH-DSA-SHA2-128s:
  `crates/node/src/crypto/hybrid.rs:128`.
- A wgpu compute pattern with a CPU oracle, for mining only: `hal/wgpu-miner`.
- No GPU signature code.

## How the Maya2C design differs

- **Advisory, not authoritative.** In the surveyed chain use (Solana) the GPU
  result is the verification. ADR-137 lets a GPU answer be compared (G1) and
  reorder pending work (G2) and nothing else.
- **wgpu/WGSL rather than CUDA**, to match the existing miner and avoid a
  one-vendor closed library on a node. WGSL has no 64-bit integers, which the
  CUDA work above does not face; the cost of that is unmeasured.
- **Verification, small batches.** The published results are mostly signing
  and large batches; a Maya2C block holds fewer than 470 hybrid signatures.

**Verdict: similar exists, and is more mature (CUDA libraries and papers for
both schemes; a production chain with GPU Ed25519). No comparative claim is
authorised. Any Maya2C speed-up is MISSING until measured.**
