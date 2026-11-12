# ADR-086: wallet-v2 — seedless smart-account recovery, clear signing, address book, payment requests

**Status:** Proposed, 2026-11-10. Wallet-only parts (§6–§8) can land now. The
consensus parts (§3–§5) need a spec section, conformance vectors
(`cargo xtask spec-coverage`), invariant 35, rust-reviewer + security-reviewer,
two human reviewers, testnet activation first ([ADR-056](ADR-056-guardian-recovery-transactions.md) answer 1), then a mainnet
activation via an [ADR-082](ADR-082-genesis-committed-upgrade-levers.md) `ScheduleUpgrade`. Nothing here is genesis-permanent.
**Extends:** [ADR-056](ADR-056-guardian-recovery-transactions.md) (corrects it, §4).
**Related:** [ADR-007](ADR-007-signature-suites.md), [013](ADR-013-suite-envelope-activation.md), [016](ADR-016-launch-scope.md), [030](ADR-030-vault-accounts.md), [036](ADR-036-chain-bound-signatures.md), [045](ADR-045-maya2c-hub.md), [046](ADR-046-universe-scale-supply.md), [050](ADR-050-owner-control-plane.md), [077](ADR-077-sentinel-recovery-network.md), [081](ADR-081-address-text-bech32m.md), [082](ADR-082-genesis-committed-upgrade-levers.md);
invariants 7, 24, 25, 28, 29, 30, 31, 32, 34.
**Catalog row:** `wallet-v2` (W1, L).

## Findings that change the brief

1. There is no `apps/wallet-gui/core/src/recovery.rs`. The core has `hd.rs`
   (SLIP-0010 mnemonic), `vault.rs`, `confirm.rs`, `payment.rs`. Today's wallet
   uses a paper seed.
2. The node cannot rotate a key and keep the address: `Transaction::sender()`
   (`crates/node/src/core/transaction.rs:242-250`) hashes the keys or the
   multisig policy, and `stage_transaction` (`crates/node/src/state/db.rs:820-825`)
   relies on that on purpose. Keeping an address across recovery needs a new
   wire version and a stored authority record — a cost [ADR-056](ADR-056-guardian-recovery-transactions.md) never states.
3. `crates/smart-account` recovery has six defects vs [ADR-056](ADR-056-guardian-recovery-transactions.md) (§4). Worst: one
   guardian proposing first can lock recovery forever (`account.rs:559-563`).
4. [ADR-056](ADR-056-guardian-recovery-transactions.md)'s 30-day maximum delay exceeds [ADR-082](ADR-082-genesis-committed-upgrade-levers.md)'s council lead (H3); the
   longest delay that lead covers is `vault::MAX_DELAY_BLOCKS` (1,000,000).
5. `BLOCKS_PER_DAY = 43_200` in `crates/smart-account/src/policy.rs:7` assumes
   2 s blocks; [ADR-056](ADR-056-guardian-recovery-transactions.md)/082 assume 1 block/s. Any "days" copy is off by 2× until settled.
6. Nothing must be fixed at genesis; every consensus part can activate later via [ADR-082](ADR-082-genesis-committed-upgrade-levers.md).

## 1. Context

| Piece | Where | State |
|---|---|---|
| Guardian/delay/cancel state machine | `crates/smart-account/src/{account,recovery,op}.rs` | RESEARCH; no node transaction reaches it |
| [ADR-056](ADR-056-guardian-recovery-transactions.md) transaction kinds | none | accepted on paper; no `TxKind` |
| Clear-sign review | `crates/clear-sign/src/lib.rs` | library, 220-case phishing set; not called by the wallet |
| Confirm screen | `apps/wallet-gui/core/src/confirm.rs` | built from `describe(tx)` (exhaustive match); hash-bound signing; hex addresses |
| Payment requests | `apps/wallet-gui/core/src/payment.rs` | `maya:<hex>?amount=&label=`; Bech32m accepted on input, hex on output |
| Address book | none | — |
| Key custody | `apps/wallet-gui/core/src/{hd,vault}.rs` | BIP-39, SLIP-0010, Argon2id-wrapped seed in the OS keychain |

## 2. Decision summary

| # | Decision | Consensus? |
|---|---|---|
| D1 | Account authority record: an address opts in to "signed for by policy P" (a stored multisig policy digest), changeable without changing the address | **Yes** — new state layer, wire v9, `stage_transaction` check |
| D2 | Device keys: each device makes its own suite-`0x30` hybrid key on-device; the account is an m-of-n policy over them; no seed generated | uses D1 + existing v8 multisig verification |
| D3 | Guardian recovery ([ADR-056](ADR-056-guardian-recovery-transactions.md) corrected): set guardians → propose → height delay with cancel → finish; replaces the authority policy, never a balance | **Yes** — `TxKind::Account` (tag 54) |
| D4 | Clear signing: fixed screen per `TxKind` from `describe()`, checked by `maya-clear-sign`; full Bech32m; poisoning defenses | no |
| D5 | Address book: local, encrypted, user-named only | no |
| D6 | Payment request v2: `maya:<bech32m>?amount=&label=&message=&exp=&id=` | no |
| D7 | Paper seed stays default until D1–D3 are active on mainnet; then seedless is default and the seed an opt-in export | no |

## 3. Consensus design (D1–D3)

### 3.1 Authority record
New layer `StateLayer::AccountAuthority` (tag 18), prefix `ka:` (verify unused
against `RECORD_LAYERS`), under the state root (invariant 25), folded only when
non-empty so a chain where nobody opts in keeps its root byte for byte; undo
journal as for generic records.

```
ka:a:<address 32> -> AuthorityRecord { policy_digest: [u8;32], since: u64 }
ka:g:<address 32> -> GuardianRecord  { guardians, threshold, delay_blocks,
                                       queued: Option<(GuardianSet, effective_at)> }
ka:r:<address 32> -> RecoveryRecord  { new_policy_digest, approvals: BTreeSet<[u8;32]>,
                                       quorum_at: Option<u64>, opened_at: u64 }
```

`policy_digest` = `MultisigPolicy::digest()` (`maya_crypto_pq::multisig`); a
single key is a 1-of-1 policy — no new cryptography.

- **Rule A (authority).** If `ka:a:<X>` exists, a transaction acts for `X` only
  as a v9 frame naming `X` whose policy digest equals the record. Every v5/v6/v7/v8
  frame whose computed `sender()` is `X` is refused (`NodeError::AuthorityMoved`),
  including one signed by the original key — this kills a thief's pre-signed
  transactions when recovery finishes.
- **Rule B.** With no record, behaviour is exactly today's. Opting in is the
  first `SetGuardians` or `AdoptPolicy`, which writes the record = the signing policy.

### 3.2 Wire version 9 — account-bound frame
`crates/node/src/core/account_tx.rs`: `version 9 ‖ account[32] ‖ MultisigAuth`.
Signing domain `maya2c/account-bound/v1`, committing to chain tag ([ADR-036](ADR-036-chain-bound-signatures.md)),
explicit account and policy digest (no cross-account or cross-chain replay).
`sender()` returns the explicit account; `Transaction::verify` refuses v9 (no
height, invariant 31 pattern); `verify_at` checks signatures under
`verification_policy()` and [ADR-082](ADR-082-genesis-committed-upgrade-levers.md) sunsets; `stage_transaction` loads
`ka:a:<sender>` before `charge_fee` and applies Rule A/B (the `db.rs:820-825`
comment rewritten). Below activation, v9 decode is refused. Wallet caps device
policies at 2-of-3 / 3-of-5; frame size is a block-bytes concern (invariant 33)
— MISSING until measured.

### 3.3 `TxKind::Account` (tag 54) — no action writes a balance

| Action | Signed by | Effect | Bounds |
|---|---|---|---|
| `AdoptPolicy { policy }` | current authority | writes/replaces `ka:a:`; with guardians set, replacement is queued for `delay_blocks` and cancelable | 1–5 signers (wallet); chain `MAX_SIGNERS` |
| `SetGuardians { guardians, threshold, delay_blocks }` | current authority | first set immediate; later change queued for the current delay (fixes F1) | 1–7 distinct, not self; strict majority; `EPOCH_BLOCKS` (3,600) ≤ delay ≤ 1,000,000 |
| `ProposeRecovery { account, new_policy }` | a guardian | opens/joins a request for that exact digest; a later proposal for a different digest moves the guardian's approval (fixes F6) | one open request; new suites admissible and not sunset before `now + delay` |
| `CancelRecovery { account }` | current authority at full threshold | deletes `ka:r:` | never halted by the guard |
| `FinishRecovery { account }` | anyone | if `quorum_at + delay ≤ height` and approvals counted against the current guardian set meet threshold: writes `ka:a:`, deletes `ka:r:` and queued changes; nonce unchanged | haltable |
| `WithdrawApproval { account }` | a guardian | removes its approval; clears `quorum_at` if below threshold | — |

Vault records (`q:`) are untouched; their guardians can still cancel requests
(invariant 32). The vault allow-list gains `TxKind::Account` (moves no value),
written into invariant 32's text.

### 3.4 Guard, describe, mempool, RPC
`Module::Account` (tag 13); cancel/withdraw map to `None` (never halted).
`describe.rs` exhaustive arms. Mempool applies Rule A at the tip. RPC
`account_authority(address)`.

### 3.5 Activation (ADR-082)
`FeatureSet::ACCOUNT_AUTHORITY = 1 << 9` (bit 8 is SHIELDED, ADR-093); KNOWN
updated; protocol version bump. Testnet schedule entry first (≤24 h run), then a
governance `ScheduleUpgrade` with ≥ `MIN_UPGRADE_LEAD_BLOCKS` (30 days). Nothing
at genesis.

### 3.6 Side benefit
`AdoptPolicy` moves an address to keys under another suite without changing the
address — the account-migration tool [ADR-082](ADR-082-genesis-committed-upgrade-levers.md) says is missing. A follow-up ADR
states it formally.

## 4. Corrections to `crates/smart-account` (pin by tests first)

The node re-implements the state machine over node state (as vault did); the
library is still fixed so it stops modelling a different rule.

| # | Where | Defect | Fix |
|---|---|---|---|
| F1 | `account.rs:392-396` | `SetGuardians` immediate | queue for current delay |
| F2 | `account.rs:589` | finalize keeps session keys | drop every key on finish |
| F3 | `account.rs:552`, `recovery.rs` | threshold 0 → one guardian recovers; threshold > n impossible; delay 0 allowed | validate with §3.3 bounds |
| F4 | `account.rs:571-585` | removed guardians' approvals count | count current guardians only |
| F5 | `account.rs:571-597` | thief-queued pending transfers survive recovery | cancel back to balance on finish |
| F6 | `account.rs:559-563` | first proposer fixes the key; one guardian can lock recovery forever | approval follows latest proposal; per-digest requests; `WithdrawApproval` |
| F7 | `policy.rs:7` | `BLOCKS_PER_DAY = 43_200` | one constant from the node's block target; decide before any "days" copy |

## 5. Threat model

| Attacker | Result | Residual |
|---|---|---|
| Steals one device of 2-of-3 | below threshold; owner `AdoptPolicy` removes it | — |
| Steals every device | indistinguishable from owner; can spend and cancel recovery (stated in UI) | vault limits, separated devices |
| One device then lockout attempt | needs threshold; changes are queued; owner banner + cancel | must notice within delay |
| Guardian quorum colludes while owner away | succeeds after delay if not cancelled | [ADR-056](ADR-056-guardian-recovery-transactions.md)'s accepted trade |
| Hostile guardian proposes junk | F6 fix: majority outvotes | — |
| Guardian key stolen | one approval < strict majority | — |
| Pre-signed old-key frames after recovery | Rule A refuses | — |
| v9 replay cross-account/chain | domain + chain tag + account + digest | — |
| Suite broken mid-recovery | suites admissible to `now + delay`; delay ≤ council lead | — |
| Block-bytes DoS | invariant 33 + per-byte fee | measure |
| Guard halts recovery | cancel/withdraw never halted | — |
| Address poisoning | §6.2 | screenshot typing |
| Malicious dApp | confirm computed from signed bytes; claim vs simulation | — |
| QR/URI injection | Rust parsing, checksum + HRP, label shown as a claim | — |
| Seedless user loses everything | onboarding requires ≥2 devices and ≥3 guardians, or a seed export | user who declines both |

## 6. Clear signing (D4)

All text produced in Rust (`confirm.rs` → `approval_message`), never in the
webview. Header on every screen: network name, chain-id prefix, sender in full
Bech32m, nonce, fee in MAYA, fee-exceeds-amount warning; then `review`
warnings, most severe first.

| Kind | Body | Forced warnings |
|---|---|---|
| Transfer | each output: full Bech32m + book name + amount | `LookAlikeRecipient`, `DrainsBalance`, first payment to address |
| Asset / RWA / DvP / revenue | asset id + chain-sourced symbol, amount with decimals, counterparty | `DrainsBalance` |
| Swap / route / order / liquidity / pool | pay X, receive at least Y, pool, slippage bound | unbounded min-out |
| Call / deploy contract | contract (verified?), method, value, simulated effects | `UnverifiedContract`, `ClaimMismatch` |
| HTLC / channels | escrow, expiry height + approx duration, counterparty | — |
| Stake | amount, unlock height, validator | — |
| Vault | delay, limit, guardians / request to, amount, ready height | `ControlChange` on configure |
| Account: AdoptPolicy | old → new signer count, device names, queue notice | `ControlChange` (top) |
| Account: SetGuardians | full list, threshold, delay blocks ≈ days | `ControlChange` |
| Account: ProposeRecovery | "You approve that account X be controlled by new keys…", digest prefix, confirm out of band | `ControlChange` |
| Account: CancelRecovery | which request, proposer | — |
| DID / attestation / governance / council | every `describe()` value in full | `ControlChange` for key/council actions |

`maya-clear-sign` work: `Effect::SetAuthority` and `Effect::SetGuardians`
(raise `ControlChange`); `render` shows full Bech32m via `maya-address`; the
`DrainsBalance` test (`lib.rs:217`) uses 512-bit `mul_wide` — rewrite within 256
bits (`amount*10 >= balance*9` with `checked_mul`, else
`amount >= balance - balance/10`), pinned by a boundary test (2026-11-01 rule).

### 6.1 Address display (ADR-081)
Full Bech32m in 4-character groups, HRP coloured and named; hex never on a
confirm screen; truncation only in lists and never the source of a copy.

### 6.2 Poisoning defenses
1. Look-alike check on displayed text: first 6 + last 6 data characters
   matching a book entry or recent counterparty raises `LookAlikeRecipient`
   (60 bits vs today's 32-bit byte check).
2. Dust and never-paid incoming transfers hidden from "recent", no copy button.
3. Send form fills only from book, scanned QR or paste; paste checked against book.
4. Mandatory "new recipient" interstitial with full read-back.
5. Bech32m checksum and HRP stop typos and wrong-network sends.

## 7. Address book (D5) and payment request v2 (D6)

`apps/wallet-gui/core/src/address_book.rs`, one record encrypted under the
vault's Argon2id key in the existing `SecretStore`.
`Contact { address: [u8;32], network, name (user-typed only), added_at_height,
source: Manual|Scanned|PaidTo, note }`. Request labels are suggestions marked
"claimed by sender". Names NFC-normalized, bidi controls stripped, confusable
warning. Feeds `clear_sign::Context.contacts`. Encrypted export/import; no
cloud sync in v2.

`payment.rs` v2, backward compatible (unknown params ignored, hex input still
accepted): `maya:<bech32m>?amount=<base units>&label=&message=&exp=<unix s>&id=<≤32>`.
Output always Bech32m; network from HRP (other network refused before the
form opens); `exp` is wallet wall-clock (not consensus); `id` matched locally.
Signed requests deferred.

## 8. Seedless onboarding (D7)

Before activation: unchanged (mnemonic); device-key screen shows "available
after the network upgrade at height H" from the `up:` schedule. After: on-device
`0x30` key (OS keystore, `ZeroizeOnDrop`), QR pairing of a second device
(`airgap.rs` frames carry public keys only), 3–5 guardians from the book,
`AdoptPolicy` 2-of-n + `SetGuardians` with a 7-day delay. Onboarding refuses to
finish seedless without ≥2 devices and ≥3 guardians, or a seed export. Existing
mnemonic accounts upgrade in place. Persistent red banner with one-tap cancel
while a recovery or queued change is open; optional ntfy push.

## 9. Test plan

Consensus (`crates/node/tests/account_authority_tests.rs`):
no change before quorum+delay; cancel ends request; old-key frames refused after
recovery (v6, v7, v8, stale v9); actions write no balance; SetGuardians queued
(F1); one guardian cannot block a majority (F6); removed guardians' approvals
ignored (F4); guardian bounds (F3); cancel never halted; v9 refused below
activation and by `verify`; v9 signing bytes bind account/policy/chain; suite
sunset before finish refused; root unchanged when nobody opts in; reorg restores
records; vault limits survive recovery; measured v9 frame bytes (2-of-3, 3-of-5)
printed to `reports/`. Conformance `spec/account-authority-vectors.json`.
Library: F1–F6 regressions in `crates/smart-account/tests/account_tests.rs`.
Wallet: confirm per kind with full Bech32m; confirm feeds `review`; Bech32m
poisoning family in the phishing set and a 256-bit drain boundary (report the
measured false-alarm rate); address book encryption, label ≠ name, confusables,
dust exclusion; payment v2 round-trip, wrong HRP, expiry, old hex URIs, unknown
params. Red team: scripted one-device thief tries every door; best attempt kept as a test.

## 10. Phased build plan

| Phase | Scope | Consensus | Done when |
|---|---|---|---|
| P0 now | Bech32m display, confirm covers every kind, `review` wired into confirm, 256-bit drain check (`confirm.rs`, `payment.rs`, `crates/clear-sign/src/lib.rs`, tests) | no | tests; clear-sign row `working` with test names |
| P1 | Address book, poisoning defenses, payment v2, GUI views (`apps/wallet-gui/src/`) | no | wallet tests; site page updated |
| P2 | F1–F7 in `crates/smart-account` | no (RESEARCH) | regressions pass |
| P3 dark | layer 18, tag 54, wire v9, Rule A/B, guard module, describe, RPC, vectors, invariant 35 text (new `state/account_authority.rs`, `core/account_payload.rs`, `core/account_tx.rs`; edits to `payload.rs`, `transaction.rs`, `db.rs`, `proof.rs`, `commitments.rs`, `invariant_guard/mod.rs`, `describe.rs`, `upgrade.rs`, mempool, RPC) | yes, inactive | all consensus tests; two reviews; two humans |
| P4 | testnet activation, ≤24 h, red team | testnet | real output in reports |
| P5 | seedless wallet (`device_key.rs`, `recovery.rs`, `airgap.rs`, `wallet.rs` v9 signing, GUI) | uses P3 | E2E on testnet: create → pair → guardians → lose devices → recover |
| P6 | mainnet `ScheduleUpgrade`, lead ≥ 30 days | yes | **Eric's APPROVED** |

## 11. Prior art (from knowledge — a dated search in `docs/prior-art/smart-account-recovery.md` is required before any public claim)

| System | What it does | How Maya2C would differ |
|---|---|---|
| Argent | guardians, ~36–48 h security period, owner cancel, guardian lock | height-based delay; strict majority enforced by the chain; replaces a PQ multisig policy; no guardian lock power |
| Safe + recovery modules | m-of-n contract wallet, recovery via modules | protocol transaction with fixed meaning, no per-wallet module a drainer can target; `ControlChange` + simulated effects |
| ERC-4337 | custom validation per account | one reviewed state machine for all accounts; no bundler layer |
| EIP-7702 | EOA gets code; original key keeps control | Rule A refuses the original key after recovery |
| Starknet / zkSync native AA | native AA with classical/STARK keys | not compared in detail; aim: hybrid ML-DSA-65+SLH-DSA under every account |

## 12. Rejected alternatives
Sweep to a new address (thief keeps the old one; every value door needs its own
sweep). Linking `crates/smart-account` into the node (own registry/balances).
Guardian lock power (instant griefing lever). Wall-clock delays (Standing Order
4). 30-day maximum (exceeds the council lead). Signed payment requests in v2
(useful only with DID binding). Seedless before activation (one lost device
loses funds).

## 13. Consequences
`stage_transaction` becomes stateful for opted-in addresses; light clients need
a state proof of `ka:a:` (BACKLOG item; same class as [ADR-082](ADR-082-genesis-committed-upgrade-levers.md) M2). Invariant 35
once tests pin it: "An address's authority changes only by its own authority
(queued under guardians) or by a guardian quorum after `delay_blocks`; never
after a cancel; and no account action writes a balance." New `features.toml`
subsystem `account-authority`; architecture-vision entry before P3 touches
`Cargo.toml`.

## 14. Questions for Eric
1. Block time for "days": 1 s ([ADR-082](ADR-082-genesis-committed-upgrade-levers.md)) or 2 s (`smart-account` policy)? (F7)
2. Cap recovery delay at 1,000,000 blocks instead of [ADR-056](ADR-056-guardian-recovery-transactions.md)'s 30 days?
3. Guardians with no lock power (recommended) or a capped lock?
4. Accept that opted-in accounts become stateful to verify (light-client gap)?
