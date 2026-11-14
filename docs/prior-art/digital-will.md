# Prior art: Digital will (assets that pass to heirs after the owner stops signing)

**Search status:** searched 2026-11-10 (web search; sources cited below).

Feature: CATALOG §I row `digital-will` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R0 idea. Depends on the inheritance item (B) in the same catalog.

**Queries:** "crypto inheritance smart contract dead man switch digital will 2026"; "Sarcophagus dead man's switch decentralized encrypted data release".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Webacy Crypto Will | Live service. The switch fires 14 months after no wallet activity; beneficiaries then have 12 months to claim. An owner who returns can reverse the transfer. | [Webacy docs](https://docs.webacy.com/product-guides/crypto-will) |
| DeadSwitch (ETHGlobal Cannes 2026) | Prototype. Owner deploys a contract, registers heirs with World ID verification, sets an inactivity threshold. A 30-day recovery delay cancels false triggers. | [ETHGlobal showcase](https://ethglobal.com/showcase/deadswitch-gcu1m) |
| Chainlink hackathon "DeadmanSwitch AI" and Monad "DeadMan" | Prototypes. An off-chain workflow triggers the transfer after inactivity; signed delegations activate at a deadline. | [Chainlink hack-26](https://chain.link/hack-26/projects/deadmanswitch-ai), [HackQuest](https://hackquest.io/projects/DeadMan) |
| Sarcophagus | Dead man's switch for encrypted data (not assets); see [time-capsule.md](time-capsule.md). | [techlore forum](https://discuss.techlore.tech/t/sarcophagus-ethereum-dead-mans-switch-for-secret-digital-data-sharing/5597) |
| Safe modules, Argent-style guardians | Account-level recovery modules with delays. Safe typically 1 to 7 days; Argent's delay is reported as 3 days in one source and 48 hours in another, unresolved. | [ChainScore comparison](https://www.chainscorelabs.com/comparisons/wallets-eoa-vs-smart-contract-wallets/recovery-and-security-policies/recovery-delay-periods-vs-immediate-access), [Haqq](https://www.haqq.network/blog/social-recovery-wallets) |

Open problem noted in the sources: the chain cannot observe a death. Projects use check-ins, oracles, or proposed proofs of death. One 2017 interview warns an inheritance switch invites attackers to keep the owner off the network. No legal validity information was found.

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/smart-account/src/recovery.rs`: guardians, a delay (`delay_blocks`), and a cancel window in which any current owner key can cancel a proposal.
- `crates/smart-account/src/policy.rs`: `Policy` with delayed transfers above a threshold (`delay_above`, `delay_blocks`).
- Heights, not timestamps (invariant 9): `crates/htlc-lattice/src/timelock.rs`.

## How a Maya2C version would differ

- **Native account, no separate contract.** An inactivity trigger would be a policy on the smart account (`recovery.rs` pattern), not a deployed escrow contract per estate.
- **Height-based inactivity.** The trigger counts blocks, so it cannot be moved by a miner's timestamp.
- **Cancel window reused.** The same cancel-by-any-owner-key rule as recovery, so a false trigger is cancellable while the owner is still active.

Not built: the inactivity trigger, heir registration, and any death-verification path. Those are the open parts and are not claimed.

**Verdict: similar exists (Webacy, Safe and Argent recovery, several prototypes). Differentiate by a native-account, height-based inactivity trigger with the existing cancel window. Death verification remains open.**

**forbidden**: "first crypto will", "only digital inheritance on chain", "legally binding digital will".
