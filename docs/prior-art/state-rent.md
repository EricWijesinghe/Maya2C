# Prior art: State rent, storage deposits and state expiry

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "Solana rent-exempt minimum balance 2026 NEAR storage staking Ethereum state expiry EIP-7736 state rent proposal"). Most hits are glossary and vendor pages; the Anza protocol doc is primary. Figures marked (vendor) were not checked against a primary source.

Feature: parity row P-25, [ADR-131](../adr/ADR-131-state-rent-or-storage-deposit.md) (OPTIONS FOR ERIC).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Solana | An account must hold a rent-exempt minimum: two years of rent at a per-byte rate over (account size + 128 bytes), refundable when the account is closed. Time-based rent collection is not the live model; exemption is required. One vendor doc says the per-byte rate was 6,960 lamports from launch until a 2026 reduction (SIMD-0437, vendor; unverified against the SIMD). | [Anza: rent](https://docs.anza.xyz/implemented-proposals/rent), [QuickNode guide (vendor)](https://www.quicknode.com/guides/solana-development/getting-started/understanding-rent-on-solana), [Chainstack rent reduction (vendor)](https://docs.chainstack.com/docs/solana-rent-reduction-reclaim-excess-rent.md) |
| NEAR | Planned time-based state rent, deprecated it before mainnet; uses storage staking: tokens locked per byte stored, earning no staking reward, returned when the data is deleted. | [spark.money glossary (secondary)](https://www.spark.money/glossary/state-rent) |
| Ethereum | No rent or expiry shipped. EIP-7736 (leaf-level expiry on Verkle stems with a revival proof) is a proposal; the two sources describe it differently and neither is the EIP text. EIP-4444 is history expiry, not state. One source says Ethereum Foundation researchers prefer partial nodes and distributed storage over consensus-level expiry. | [spark.money: state expiry (secondary)](https://www.spark.money/glossary/state-expiry), [chainscore (opinion)](https://www.chainscorelabs.com/blog/zk-rollups-the-endgame-for-scaling/state-management-and-storage/why-state-rent-is-a-political-minefield-not-a-technical-fix) |

Not found: a primary EIP-7736 text, the SIMD-0437 text, a chain that ships lazy per-block rent today.

## What Maya2C has (checked in repo, 2026-11-12)

- No rent, deposit or expiry anywhere; `rg rent` over `docs/architecture-vision.md`, `docs/invariants.md`, `BACKLOG.md` finds nothing on the topic.
- Byte-priced base fee at inclusion (`crates/fee-market/src/limits.rs:36,60`, ADR-029); a 40-byte account record never deleted (`crates/node/src/state/account.rs:18,26-32`); contract storage bounded per key/value (`crates/vm/src/host.rs:28-31`) with the whole key space read before a call (`crates/node/src/state/contracts.rs:9-16`).
- History pruning with archive-before-delete (`crates/node/src/state_pruner/mod.rs:5-31`, invariant 27); nothing prunes state.
- ADR-123 (Proposed) gives *blob bytes* an expiry; records under the root have none.

## How a Maya2C version would differ

- A deposit (ADR-131 option C) would be a record beside the account under a new `rent:` prefix (invariant 25), so the 40-byte account encoding and every pre-activation account are untouched; Solana stores the exemption as the account's own lamports.
- Any multiplication is bounded in writing and done in checked `U256` with no 512-bit intermediate (ADR-046; memory rule 2026-11-01).
- Expiry (option D) would need a revival witness; the stateless path that could supply one is RESEARCH (`STATELESS_ACTIVATION_HEIGHT = u64::MAX`).

**What may be said publicly:** nothing yet; no option is chosen. If C is adopted: "a refundable storage deposit, the model Solana and NEAR use". **Forbidden:** "first", "only", "novel state pricing".

**Verdict:** similar exists on every surveyed chain that has it; the deposit shape is the industry convergence. Options, not a design; Eric decides.
