# ADR-050: The owner's control plane — kill switches without a backdoor

**Status:** Proposed, 2026-10-18. Waiting on Eric's decisions (see the end).

## Context

Eric asked for "built-in mechanisms where you and me have full access, including kill switches and every enterprise-grade security feature to have full control".

A public chain has two very different things that could be called "control":

1. **Control of what we run.** Our nodes, the websites, the RPC, the faucet, the release pipeline, and the accounts behind them (GitHub, Cloudflare, Netlify). Full control here is good and expected.
2. **Control of the chain itself.** Freezing or moving other people's coins, or rewriting history. A hidden master key that can do this is a **backdoor**. It would be:
   - the most valuable target for every attacker, and a stolen Cloudflare token already showed (2026-10-06) that our accounts are attacked;
   - a reason for exchanges, investors and users to refuse the chain;
   - a legal liability for whoever holds it.

So the answer is: **full, strong control over layer 1, and over layer 2 only bounded, visible, multi-signature emergency brakes, never a master key.**

**What exists today** (`crates/node/src/state/council.rs`, `core/council_payload.rs`, `genesis.rs` `CouncilGenesis`):

- **A security council at genesis:** ML-DSA-65 member keys and a strict-majority threshold.
- **Its one power:**
  - It pauses **one module** for at most `max_pause_blocks`, or ends a pause early.
  - The modules are Channels, Dex, Shielded, Oracle, Governance, Vm, Sealed, Identity, Rwa, Htlc, ThreatIntel, Iot and Vault.
  - It **cannot** move funds, mint or rewrite blocks.
- **The automatic circuit breaker** (`invariant_guard`) halts a module after an invariant anomaly.
- **The live testnet genesis has no `security_council`,** so today nobody can pause anything.

## Decision (proposed)

### Layer 1 — operator control (ours, full)

| Control | Mechanism | State |
|---|---|---|
| Stop or start our validators | Watchdog scripts; stop by PID and install path only (never by image name) | exists |
| Validator keys | Sealed at rest (ADR-049); remote signer option (ADR-033) | PR #131 |
| RPC and faucet abuse | Per-IP limiter, 429 | exists |
| Site integrity | Hourly integrity watcher, strict CSP | exists |
| Releases | SHA256SUMS, SBOM, signed provenance | PR #130 |
| Accounts | Hardware security keys (FIDO2) on GitHub, Cloudflare, Netlify, email; scoped short-lived API tokens; no long-lived tokens on disk | **Eric** |
| Disk at rest | BitLocker on C: and D:, recovery key on paper | **Eric** |

### Layer 2 — protocol emergency brake (bounded, visible, multi-signature)

- **Mainnet genesis includes a `security_council`** of 3 ML-DSA-65 keys with threshold 2, each on a different device:
  1. this PC;
  2. the MacBook when it arrives;
  3. an offline paper or hardware backup in a safe place.

  No single stolen device can pause anything, and losing one device locks nobody out.
- **Bounded pause.** `max_pause_blocks` is set to a few hours of blocks. A pause is an on-chain, public transaction; it expires on its own, and renewing it needs a fresh 2-of-3.
- **Nothing more.** No freeze of balances, no forced transfers, no history rewrite, no fee or emission change. Those go through on-chain governance later, with a timelock, and are out of this ADR.
- **The chain-wide halt** stays what it is in every BFT chain: honest validators stop signing. With the foundation set on one PC (ADR-047), that is our own watchdog.

### Claude's access

Claude works through the repository, pull requests and commands that Eric's sessions approve. **Claude never holds a council key, a validator key, a wallet key, a passphrase or an account token,** and does not deploy to mainnet without `APPROVED:`. That keeps every irreversible power with a person who can be held to it.

## Consequences

- Investors and users can be told exactly what the project can and cannot do to their funds, in one sentence that is true.
- An attacker who steals one device or one account still cannot pause the chain or touch funds.
- The council keys must be generated in the genesis ceremony (gate 5), never by Claude.

## Decisions needed from Eric

1. Council size and threshold: 2-of-3 as proposed, or 3-of-5 with trusted people.
2. `max_pause_blocks`, the longest single pause.
3. Where the third, offline key lives.
4. Whether the testnet gets a council now, which needs a re-genesis and resets the 7-day soak, or only mainnet does.

