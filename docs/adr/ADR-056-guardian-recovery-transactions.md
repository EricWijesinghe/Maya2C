# ADR-056: Guardian recovery as node transactions

**Status:** Accepted 2026-10-19 (Eric: "all decisions are approved, take the best recommended"), with the answers below. A consensus change: spec, conformance vectors, an invariant, an activation height and two reviewers before it is live.

**Answers:**
1. Testnet first, behind an activation height, rolled out one validator at a time. ADR-016 makes smart-account validation launch core, so mainnet must not be its first chain.
2. Default delay 7 days; the minimum stays one epoch (3,600 blocks).
3. Vaults stay separate. Invariant 32 stays as simple as it is; a vault owner who wants recovery puts the vault under a recoverable account.

## Context

Queue item 6 asked for wallet guardian recovery: lose every key, get the account back through people you trust. The logic exists in `crates/smart-account/src/recovery.rs` (RESEARCH): a `GuardianSet` (accounts, threshold, `delay_blocks`), and a `RecoveryRequest` that takes effect only after the delay, during which any current owner key can cancel it. ADR-016 already lists smart-account validation as launch core. **What is missing is the node side:** no transaction kind carries any of it, so no wallet can use it.

Vault guardians (ADR-030) are different. They can only *cancel* a vault withdrawal. They can never change a key.

## Decision (proposed)

Four transaction kinds, each signed by an ordinary hybrid PQ account, so no new cryptography:

| Kind | Signed by | Effect |
|---|---|---|
| `SetGuardians { guardians, threshold, delay_blocks }` | the owner | replaces the set; takes effect after the *current* set's delay, so a thief with the owner key cannot instantly remove the guardians |
| `ProposeRecovery { account, new_key }` | a guardian | opens or joins a request for that exact key |
| `CancelRecovery { account }` | the owner (current key) | ends any open request |
| `FinishRecovery { account }` | anyone | after quorum + `delay_blocks`, rotates the key; the address stays the same |

Bounds (proposed): 1–7 guardians, threshold a strict majority, `delay_blocks` at least one epoch (3,600 blocks) and at most 30 days of blocks. One open request per account. State under a new prefix (invariant 25), with a numbered invariant once a test pins "no key change before quorum + delay, and none at all after a cancel".

Wallet: a "Recovery" screen with three guardians by address, and a banner on every device of the owner while a request is open, because the cancel window only protects an owner who notices.

## Risks

- Guardians colluding while the owner is away for longer than the delay. That is the trade: a longer delay is safer and a slower recovery.
- A guardian whose own key is stolen. The threshold covers one; the owner banner covers the rest.

## Questions for Eric

1. Testnet now, with an activation height and a rolling upgrade one validator at a time, or only at mainnet genesis?
2. The delay default: 1 epoch, or 7 days?
3. Should recovery also be able to rotate a *vault* key (ADR-030), or vaults stay separate?
