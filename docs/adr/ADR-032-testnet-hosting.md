# ADR-032: First public testnet — one seed on Oracle Always Free, one installer

**Status:** Proposed. The owner chose the hosting on 2026-09-29; nothing has
been applied (`APPROVED: testnet seed` is still outstanding).
**Date:** 2026-09-29

## Context

The owner has bought `maya2c.dev` and wants the network usable by outsiders.
The repository's deployment pipeline (`infra/terraform/`) provisions twelve
`r6i.4xlarge` nodes across three AWS regions, which `infra/README.md` prices at
about $9,000 a month. The project has no funding for hosting, so that
pipeline cannot be the first step.

Three facts shape what the first step can be:

1. **The launch consensus is DAG-BFT** (ADR-016, ADR-027). Proof of work is a
   devnet mode, refused by a production build. A public network therefore needs
   validators, not miners.
2. **DAG-BFT liveness depends on committee size.** A committee of *n*
   tolerates ⌊(*n*−1)/3⌋ faults. With *n* = 1 or 2 no fault is tolerated; with
   *n* = 2 *either* machine stopping halts the chain.
3. **No binary release exists**, and publishing one needs its own approval.

## Decision

- **Host the first seed on Oracle Cloud's Always Free tier**: one Ampere A1
  VM (4 OCPU, 24 GB, aarch64) at $0, provisioned by `infra/oracle-free/`.
  Variable validation refuses shapes beyond the free limits, SSH open to the
  world, a branch in place of a commit, and a mainnet chain id.
- **One installer for every host.** `infra/oracle-free` only provisions; the
  VM's cloud-init clones the repository at a **pinned full commit SHA**,
  verifies it, and runs `infra/testnet-vm/install.sh` (built by the parallel
  deploy session). The same script's JOIN mode is how a home PC joins.
  Keeping two installers would give two things that drift.
- **Build from source on the VM** rather than downloading a release, so the
  testnet does not wait on a publishing approval, and the VM runs exactly the
  tree that commit names.
- **Start with a committee of one; the home PC joins as an observer.** Adding
  it as a second validator would make liveness worse (fact 2). Growth to four
  independent validators goes through the staking registration path, which is
  built and tested (`crates/node/tests/bft_staking_tests.rs`).
- **The website stays on GitHub Pages** (free, public repository), with DNS in
  Cloudflare set to DNS-only so GitHub can issue the certificate.

## Consequences

- The testnet is **centralised and fragile** until there are four validators
  run by independent people: one VM, one provider, one operator. The site and
  guides say so.
- Oracle can reclaim idle Always Free instances or suspend an account. State
  and the validator key live on one boot volume; `/etc/maya2c` must be backed
  up off the VM.
- A first build of the node on four Ampere cores is slow (not yet timed on
  aarch64; the x86 release build took 12m14s on two jobs), and nobody has yet
  compiled the node for aarch64. That is the first thing the first apply
  tests.
- `infra/terraform/` stays as the target for a funded network; this does not
  replace it.

## Alternatives considered

- **AWS fleet (`infra/terraform/`)** — about $9,000 a month; rejected for now on cost.
- **A €5–15/month VPS** — simpler, x86, reliable; the owner chose $0.
- **Home PC only** — $0, but not always online, and it exposes a home IP as the
  seed.
- **Four validators on the one Oracle VM** — survives a process crash, not the
  loss of the VM, so it would imply fault tolerance that does not exist.
