<p align="center">
  <img src="../logo-assets/website/Header-Logo_250x100.png" width="250" height="100" alt="Maya2C"/>
</p>

<p align="center">
  Documentation index.
</p>

---

These documents exist because the reasoning behind a decision is the part that
does not survive in the code. Each one is written to be read by whoever
inherits the thing it describes.

## Read these first

| Document | What it settles |
|---|---|
| [architecture-vision.md](architecture-vision.md) | The full target architecture, and which of it is shipped, which is dark, and which was never written |
| [trajectory.md](trajectory.md) | The 160-prompt build order, where the sessions are in it, and where the plan disagrees with the tree |
| [mainnet-readiness.md](mainnet-readiness.md) | Why mainnet is blocked, and what lifting the block would require |
| [launch-checklist.md](launch-checklist.md) | What is verified, what is generated but unverified, and what is outstanding |
| [security-audit.md](security-audit.md) | The audit surface and its findings |

## Consensus and cryptography

| Document | What it settles |
|---|---|
| [hybrid-signatures.md](hybrid-signatures.md) | Two signatures per transaction, why both must verify, and why signing is deterministic |
| [dag-pow.md](dag-pow.md) | The Ethash-style DAG proof of work and its activation |
| [lattice-pow.md](lattice-pow.md) | Lattice proof-of-useful-work. A research branch — nothing in consensus calls it |
| [pq-transport.md](pq-transport.md) | ML-KEM-768 over Noise, and the conditions for enabling the HQC second KEM |
| [radio-transport.md](radio-transport.md) | Why the off-grid link carries headers and not transactions, and why its governor refuses rather than warns |
| [oracle.md](oracle.md) | Why freshness is measured in block height and never in timestamps |
| [governance.md](governance.md) | The bounds a proposal may never escape, and why no governed value is a program |
| [identity.md](identity.md) | Why a DID is an address rather than a key, and why the attestation is post-quantum but the disclosure proof is not |
| [invariant-guard.md](invariant-guard.md) | Why a block that creates value is invalid, why a merely alarming one only halts its module, and why no breaker can stop a transfer |

## Execution and markets

| Document | What it settles |
|---|---|
| [dex.md](dex.md) | Why a losing trade is a no-op and never an error |
| [vm-module-cache.md](vm-module-cache.md) | Why the VM was already a JIT, and why a cache keyed on bytecode alone could fork a chain |
| [rwa.md](rwa.md) | Why a DvP that cannot settle is a no-op, and why pro-rata dust would fail the invariant guard |
| [iso20022.md](iso20022.md) | Bank-rail messages: why an inexact amount is refused rather than rounded, and how a compliance check costs no anonymity |
| [blockgraph.md](blockgraph.md) | Batch references and deterministic shard scheduling. Also a research branch |

## Mining and pools

| Document | What it settles |
|---|---|
| [stratum-v2.md](stratum-v2.md) | The pool protocol, fuzzable without the chain |
| [pool-service.md](pool-service.md) | PPLNS accounting and treasury payouts |
| [wgpu-miner.md](wgpu-miner.md) | The cross-platform GPU miner, and what it does not yet do |

## Services

| Document | What it settles |
|---|---|
| [faucet.md](faucet.md) | Why the two rate-limit buckets are independent, and why the daily cap is the control that matters |
| [telemetry.md](telemetry.md) | Why nothing on the dashboard is verified, and why the map is country-granular |
| [sdk.md](sdk.md) | The multi-language SDK surface, and which bindings are generated but never compiled |

## Economics

| Document | What it settles |
|---|---|
| [fee-market.md](fee-market.md) | A byte-priced EIP-1559 base fee, where each fee goes, and why the supply cap bounds a supply nothing yet increases |

## Custody

| Document | What it settles |
|---|---|
| [custody-mpc.md](custody-mpc.md) | Why institutional threshold custody protects the 32-byte chain key rather than thresholding either signature, and where the trust boundary actually sits |
| [zkml.md](zkml.md) | Verifying proofs of model inference in a contract: what is measured, why it is dark, and why re-running the model is still cheaper |
| [ledger-feasibility.md](ledger-feasibility.md) | Whether a Ledger can sign a Maya2C transaction — no, and the obstacle is the hash-based half |
| [pruning.md](pruning.md) | Dropping old block bodies once a verified archive exists, fetching them back, and bootstrapping a node from a state snapshot |

## Production, launch and adoption (Master Prompts 11–30)

| Document | What it settles |
|---|---|
| [master-prompts/README.md](master-prompts/README.md) | For each of the thirty briefs in `Prompts/`: whether its DONE WHEN is met, and the largest gap |
| [NODE_TYPES.md](NODE_TYPES.md) | Measured resource requirements per node type, and what was not measured |
| [SECOND_CLIENT.md](SECOND_CLIENT.md), [CRYPTO_WATCH.md](CRYPTO_WATCH.md) | Why a spec with an independent verifier comes first, and what cryptanalysis would trigger a migration |
| [ECONOMIC_SECURITY.md](ECONOMIC_SECURITY.md) | Cost-of-attack tables from the economic scenarios |
| [SLO.md](SLO.md), [runbooks/](runbooks/README.md), [TESTNET_PROGRAM.md](TESTNET_PROGRAM.md) | Service objectives, the 20 runbooks, and the testnet phases |
| [BENCHMARK_METHODOLOGY.md](BENCHMARK_METHODOLOGY.md) | The rules every published figure follows |
| [GENESIS.md](GENESIS.md), [ROADMAP.md](ROADMAP.md), [audit/](audit/README.md) | The genesis rehearsal, the first 90 days, audit packets and known issues |
| [integrations/](integrations/EXCHANGES.md) | What exchanges and custodians need, including confirmation depth |
| [strategy/](strategy/WEAKNESS_MAP.md), [prior-art/](prior-art/README.md) | Sourced weaknesses, beat bars, and the searches that decide what may be called "first" |
| [privacy/LIMITS.md](privacy/LIMITS.md), [security/Q_DAY_PLAYBOOK.md](security/Q_DAY_PLAYBOOK.md) | What the privacy layer does not hide; what to do on Q-day |
| [migration/](migration/README.md) | Porting from Ethereum, Solana and Move, and the first thing that will not port |
| [ecosystem/](ecosystem/METRICS.md) | Grants, hackathon kit and course drafts, and the metrics method |
| [public/](public/BENCHMARK_REPORT.md) | Benchmark report, beat-bar status and announcement copy — drafts awaiting approval |

## Not Markdown

- `reference.tex` — the LaTeX technical reference, generated by the `docgen`
  crate from module documentation. Generated, never compiled; see
  [launch-checklist.md](launch-checklist.md).
- `grafana/` — dashboard definitions for the node's Prometheus exporter.
- `benchmarks/` — comparative benchmark output.
