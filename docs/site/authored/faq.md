---
title: FAQ
description: Direct answers about Maya2C: what it is, which post-quantum algorithms it uses, how it reaches finality, mining, whether mainnet is live, the audit status, and how to run a node or a wallet.
---

Each answer states today's status and links to the page with the detail.
Where a claim has a source, the source is named.

## What is Maya2C?

Maya2C is a layer-1 blockchain written in Rust. Its transactions are signed with
NIST post-quantum signature schemes, and its validators order blocks with a
DAG-BFT consensus. The source is public under Apache-2.0
([github.com/EricWijesinghe/Maya2C](https://github.com/EricWijesinghe/Maya2C)).

The public testnet, `maya-testnet-1`, is **paused**, and mainnet has **not launched**.
See [Join the testnet](/guides/testnet/) for the current status.

## Is Maya2C quantum-resistant, and which algorithms does it use?

Maya2C uses NIST post-quantum algorithms. No system can promise security against every
future attack, so read the parameter sets and assumptions on the signatures page.

- **Transaction signatures:** every transaction carries two signatures, ML-DSA-65
  (FIPS 204) and SLH-DSA-SHA2-128s (FIPS 205). A transaction is accepted only if
  **both** verify. The lattice and hash-based schemes rest on different hardness
  assumptions. See [Post-quantum signatures](/guides/signatures/) and
  [Hybrid signatures](/reference/hybrid-signatures/).
- **Peer connections:** key exchange uses ML-KEM (FIPS 203). ML-KEM-1024 is
  preferred, and ML-KEM-768 is still accepted for older peers. See
  [Post-quantum transport](/reference/pq-transport/).
- **Validator votes:** signed with ML-DSA. See [Consensus and finality](/guides/consensus/).

## How does Maya2C reach finality?

Validators build a certified DAG of blocks. A commit rule (Bullshark) picks an
anchor, and each committed anchor produces one block. Honest validators commit the
same anchors in the same order, so a committed block is not reorganised.

- **Fault bound:** safety holds while at most `f` of `n` validators are Byzantine,
  where `f = (n − 1) / 3` rounded down. Liveness needs partial synchrony.
- **Latency:** no finality-latency figure is published yet. The repository has no
  finalized-throughput or finality-latency benchmark. Live numbers are on the
  [status page](https://status.maya2c.dev/).
- Details and the quorum math: [Consensus and finality](/guides/consensus/).

## Can I mine Maya2C, and with what hardware?

GPU mining is built. The GPU miners are for NVIDIA GPUs with CUDA, and for any
GPU with Vulkan, Metal or DX12 through wgpu.

- Mining is a **reward lane**, separate from consensus. Miners never propose, order
  or finalize a block. See [Mining and validators](/guides/mining/).
- Mainnet has not launched, so there is no live mining reward today. This FAQ does
  not describe reward economics.
- A local proof-of-work chain for development is available. It is not the launch
  network. See [Mining and validators](/guides/mining/).

## Is mainnet live?

**No.** `maya-testnet-1` ran for 7 days 15 hours 42 minutes without a halt. It was
stopped on purpose at 2026-10-28 07:36 UTC, so the next build can start from a new
genesis. The faucet and transfers are offline until then. The soak report is in
[`reports/sessions/2026-10-28-testnet-stopped.md`](https://github.com/EricWijesinghe/Maya2C/blob/master/reports/sessions/2026-10-28-testnet-stopped.md).

## Is the code open source, and is it audited?

**Open source:** yes. The code is Apache-2.0 at
[github.com/EricWijesinghe/Maya2C](https://github.com/EricWijesinghe/Maya2C).

**Audited:** no external security audit has been completed or reported.

- **Self-review:** the project has run its own review, for example
  [`reports/security/2026-10-25-committee-self-review.md`](https://github.com/EricWijesinghe/Maya2C/blob/master/reports/security/2026-10-25-committee-self-review.md).
  A self-review is not an audit.
- **Automated checks:** fuzzing of the decoders, Kani model checks of ledger
  arithmetic, and an unsafe-code survey. They run on every pull request and nightly.
  The document states what each check does and does not prove. See
  [Security audit](/reference/security-audit/).
- **Plan:** mainnet is to launch labelled "external audit pending" until a report
  lands ([ADR-042](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-042-solo-launch-path.md)).

## How do I run a node or a wallet?

- **Local chain (no network needed):** [Quickstart](/guides/quickstart/) builds the
  node and a wallet from source and starts a one-validator chain on your machine.
- **Node binaries:** the [releases page](https://github.com/EricWijesinghe/Maya2C/releases)
  has the builds. Windows and macOS builds are not code-signed yet, so the system
  will warn about an unknown publisher. See [Code signing policy](/guides/code-signing-policy/).
- **Joining the testnet:** [Help run the network](/guides/help-run/) and
  [Run a validator](/guides/validators/) describe joining `maya-testnet-1`. The
  testnet is paused, so those steps apply only once it restarts.
- **Wallet:** [Wallet integration](/guides/wallet/) covers how a wallet holds keys,
  signs transactions, and talks to a gateway. [How a transaction works](/guides/transactions/)
  covers the checks a transaction passes.
- **API:** [API reference](/guides/api/) lists the JSON-RPC and REST endpoints.
