# ADR-131: State rent, storage deposit, or expiry — options for pricing the state an account or record occupies

**Status:** OPTIONS FOR ERIC, 2026-11-12. Nothing is decided here. Every option is a post-genesis dark feature under ADR-082; none blocks genesis.
**Date:** 2026-11-12
**Related:** parity row P-25, [prior-art/state-rent.md](../prior-art/state-rent.md), [ADR-029](ADR-029-fee-market-live.md) (byte-priced base fee), [ADR-046](ADR-046-universe-scale-supply.md) (U256, 24 decimals), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), ADR-123 (blob expiry, branch `docs/adr-117-124-gaps`), [ADR-030](ADR-030-vault-accounts.md); invariants 17, 24, 25, 27, 28, 32

## Context

What the tree prices today, and what it does not:

- **Bytes in a block are priced; bytes that stay in state are not.** The
  fee market charges a base fee per transaction byte (`crates/fee-market/src/limits.rs:36`
  `MAX_BASE_FEE_PER_BYTE`, `:60` `MAX_BLOCK_BYTES = 5 MiB`, ADR-029). A
  transaction pays once, when it lands; the record it leaves behind costs
  nothing afterwards.
- **An account is 40 bytes and is never deleted.** `Account { balance: U256,
  nonce: u64 }` (`crates/node/src/state/account.rs:26-32`), `ACCOUNT_LEN = 40`
  (`:18`), keyed `acct:` ‖ address (`state/db.rs:43`). `get_account` returns
  `Account::default()` for an unknown address (`db.rs:347-356`), so an account
  with balance 0 and nonce 0 is indistinguishable from one that never
  existed, but a record with a nonzero nonce and zero balance stays forever.
  No deletion path was found in `account.rs` or `db.rs`.
- **Contract storage is bounded per key and per value, not per contract.**
  `MAX_KEY_BYTES = 128`, `MAX_VALUE_BYTES = 4 KiB` (`crates/vm/src/host.rs:28-31`);
  `storage_set` at `crates/vm/src/runtime.rs:842` checks sizes (`:834-837`).
  In the range read (`runtime.rs:818-846`) no per-byte fuel for a write was
  found; fuel for a write may be charged elsewhere and was NOT VERIFIED. The
  host reads a contract's **entire** storage before a call
  (`crates/node/src/state/contracts.rs:9-16`), which the module itself says
  "bounds how large a contract's storage can usefully get" — a performance
  cliff, not a price.
- **History is pruned; state is not.** A pruned node deletes block bodies
  below `PRUNE_DEPTH = 30,000` blocks after an archive holds a verified copy
  (`crates/node/src/state_pruner/mod.rs:5-31`, invariant 27). Headers are
  kept forever at 145 bytes each. Nothing prunes a state record.
- **Blobs already have an expiry.** ADR-123 (Proposed) gives blob bytes a
  `BLOB_RETENTION_BLOCKS` window and deletes them without archive. That is
  the one place the tree already says "data has a lifetime", and it applies
  to bytes outside the state root, not to records under it.
- **Growth bound by arithmetic, not measurement.** At 5 MiB per block and a
  15-second block (`state_pruner/mod.rs:15`), block bytes are at most
  5 MiB × 2,102,400 blocks ≈ 10.0 TiB a year. That is a ceiling on bodies, not a
  forecast of state. One account record is 37 bytes of key plus 40 bytes of
  value before RocksDB overhead; 10⁹ accounts ≈ 77 GB. The measured state
  size of the testnet is MISSING from every report in `reports/`.

**Findings that contradict the brief.**

1. The parity row lists invariant 27 (archive before delete). Rent, deposit
   and plain expiry touch account and contract records, which 27 does not
   cover; 27 is reached only by option D's revival proof, if the revival
   witness is read from an archived state snapshot (`state_pruner/snapshot.rs`).
2. The row's Solana citation is "Solana rent". Solana's live model is the
   rent-exemption **deposit** (an account must hold two years of rent up
   front; refundable on close), not time-based rent collection; the
   prior-art file records the sources. The deposit shape is option C below.
3. "Effect on existing accounts" cannot be "none" for any option but A.
   The 40-byte record has no field for a last-paid height or a deposit, so
   either every account is re-encoded at activation (a deterministic
   migration pass that changes the state root once, invariant 24 and the
   ADR-082 schedule make that legal but it is a one-time full scan) or the
   new data lives under a new prefix beside the account (no re-encode,
   options B–D below are written that way).

## Options

### A. None: keep the byte-priced fee and pruning; do nothing to state

State growth is bounded per block by `MAX_BLOCK_BYTES` and paid for once.
Operators size disks for the arithmetic ceiling. This is Bitcoin's model
and Ethereum's model as shipped (Ethereum's researchers have said they
prefer partial nodes and distributed storage over consensus-level expiry;
prior-art file).

### B. Rent: a per-byte, per-block charge collected lazily

Each record under a rent-bearing prefix owes `bytes × rent_per_byte_block`
per block (`rent_per_byte_block` governed, invariant 17, read from state).
Rent is collected when the record is next touched: the applying transaction
pays `bytes × rate × (height − last_paid)` from the account's balance into
the fee collector, and `last_paid` is written to `rent:p:` ‖ address. An
account that cannot pay is marked delinquent (`rent:x:` ‖ address) and
refuses outgoing transfers until topped up; it is never deleted (so a
reorg has nothing to resurrect). Rent is NOT charged to accounts that
existed before activation until their first post-activation touch.

256-bit bound: `bytes ≤ 2³² ` (no record is larger than a block),
`rate ≤ 10²⁴` base units (one coin per byte-block, an absurd ceiling the
governance range enforces), `height − last_paid < 2⁶⁴`; the product is
below 2³² × 2⁸⁰ × 2⁶⁴ = 2¹⁷⁶ < 2²⁵⁶, so the multiplication is checked U256
with no 512-bit intermediate (memory rule 2026-11-01).

### C. Storage deposit: a one-time refundable bond per byte (Solana / NEAR shape)

Creating a record that did not exist locks `bytes × deposit_per_byte`
(governed) from the creating account into `rent:d:` ‖ owner → `U256`.
Deleting the record (closing an account to zero balance and zero storage,
clearing a contract key) refunds the deposit to the owner. No clock, no
per-block arithmetic, nothing to collect: the only multiplication is
`bytes × deposit_per_byte` with the bound from B. Existing accounts hold no
deposit and owe none; a new account created by a transfer after activation
needs the sender to cover the deposit in addition to the amount (a new
validity rule on `Transfer`, see invariants 28 and 32 below). A contract's
deposit is paid by the caller that writes the key and refunded to whoever
clears it, the NEAR rule.

### D. Expiry with revival: untouched records leave the tree and come back with a proof

A record untouched for `EXPIRY_BLOCKS` is replaced by a 32-byte stub
(hash of the record and its expiry height) under `rent:e:`. Reviving it
means presenting the pre-image, which the chain checks against the stub,
and paying the deposit from C. The stub keeps the state root honest
(invariant 24) and bounds the live set to what was touched recently.
Requires: an iteration over expired records per epoch (bounded per block
to keep apply time flat), and a witness source for revival, which today is
an archive node's state snapshot (`state_pruner/snapshot.rs`) or a wallet
that kept its own records. Ethereum's EIP-7736 is this shape and is not
shipped (prior-art file).

## Recommendation

**C (storage deposit), post-genesis, for new records only; A until then.**
Reasons, in order:

1. C has no clock and no collection pass: a deposit is written once and
   refunded once, so apply time per block does not change. B adds a
   subtraction to every touch and a delinquency state every wallet must
   explain; D needs the expiry sweep and a revival witness path that does
   not exist (`stateless.rs` is RESEARCH, `STATELESS_ACTIVATION_HEIGHT = u64::MAX`).
2. C is refundable, so it is a bond, not a tax. It is the model the
   surveyed chains converged on (Solana rent-exemption, NEAR storage
   staking); time-based rent was deprecated by NEAR before mainnet and by
   Solana after it.
3. C's one real cost — the sender of a transfer to a fresh address pays
   the deposit — is exactly the cost that stops address-dust spam, which is
   the attack A has no answer to other than the one-time byte fee.
4. D is the right long-term shape **if** state growth is measured to be a
   problem, and it composes with C (revival pays the deposit). Decide D
   when a measurement exists; today it is MISSING.

If Eric prefers no new rule on `Transfer` (the deposit on a fresh
recipient), the fallback is A with a measured state-size report every
sweep, and C revisited when state exceeds a figure he names.

## Consequences per option

| | A none | B rent | C deposit | D expiry |
|---|---|---|---|---|
| New state prefix | none | `rent:p:`, `rent:x:` | `rent:d:` | `rent:e:` + C's |
| Apply-time cost | none | one mul + sub per touch | one mul on create, one refund on delete | per-epoch sweep, bounded per block |
| Existing accounts | untouched | charged from first touch after activation | untouched; no deposit, none owed | expire like any other after `EXPIRY_BLOCKS` from activation (grace) |
| `Transfer` validity changes | no | yes: delinquent sender refused | yes: fresh recipient needs deposit from sender | yes: expired recipient must be revived first |
| Wallet UX | none | "your account is in arrears" | "creating this account locks X, refundable" | "this address is dormant; revive for X" |
| Spam defence | byte fee only | rent drains dust | deposit priced per dust record | dust expires |
| Reversible by governance | n/a | rate → 0 stops collection; arrears stay | rate → 0 stops new locks; refunds continue | sweep stops; stubs stay |

## Invariants touched

- **17**: `rent_per_byte_block` / `deposit_per_byte` / `EXPIRY_BLOCKS` are
  governed parameters read from state with compiled defaults, never a
  bare `const` at a call site.
- **24**: no option changes a block's roots except through records under
  the state root; option D's stub is a committed record, so an expired
  account still contributes to the root.
- **25**: `rent:` is a new prefix → `RECORD_LAYERS` (`state/commitments.rs:44`),
  a `StateLayer::Rent`, undo coverage for every write including refunds.
- **27**: only D, only if revival reads an archived snapshot; the snapshot
  path already requires `header(H).state_root` to reproduce.
- **28**: a rent/deposit/expiry **refusal** is a validity rule on the
  transaction, not a breaker. `Module::of(TxKind::Transfer)` stays `None`:
  a tripped `Module::Rent` breaker must stop deposits being *locked*, never
  stop a transfer between two existing funded accounts. Options B–D each
  add a reason a transfer can be refused; that is a change to the
  "peer-to-peer payments are ungated" reading of 28 and needs the
  invariant text amended, not quietly narrowed.
- **32**: a vault's deposit lock counts toward its window limit (it is
  value leaving the spendable balance); a refund does not.
- **Conservation (ADR-044, no native mint):** deposits are transfers into
  a per-owner record, refunds are transfers out; the guard sums `rent:d:`
  like open vault withdrawals.

## 256-bit math

All products are `bytes × rate` or `bytes × rate × blocks` with the bound
stated under option B (< 2¹⁷⁶). Checked `U256` multiplication, no 512-bit
intermediate, with the bound written as a comment at the one call site
(memory rule 2026-11-01, option A of the math decision).

## Test plan if adopted (red tests first, `crates/node/tests/state_rent_tests.rs`)

- `before_the_height_no_rent_record_exists_and_the_root_matches_master`
- `a_transfer_to_a_fresh_address_locks_the_deposit_and_the_guard_sums_it` (C)
- `closing_an_account_refunds_exactly_the_deposit_once` (C; a second close is refused)
- `a_pre_activation_account_owes_nothing_until_touched` (B) / `holds_no_deposit` (C)
- `a_delinquent_account_cannot_send_and_can_receive` (B)
- `a_vault_deposit_counts_toward_the_window_limit` (32)
- `a_reorg_restores_deposits_and_last_paid_heights` (25)
- `a_tripped_rent_breaker_refuses_new_locks_and_never_a_funded_transfer` (28)
- `the_product_bytes_rate_blocks_stays_below_2_pow_176_for_the_governed_ranges` (a const-evaluated bound, plus a Kani harness if `fee-market` holds the arithmetic)
- `an_expired_record_revives_only_with_the_matching_preimage` (D)
- `exploit_replays::dust_spam_cost_per_record_is_printed_not_asserted` — the real figure at the default rate.

Spec: `spec/` gains a "state pricing" section with vectors RENT-1..RENT-4
for whichever option is chosen; `cargo xtask spec-coverage`.

## Activation

`FeatureSet::STATE_PRICING = 1 << 37` (reserved in `adr-claims.txt` for
this ADR; unused if Eric chooses A), compiled activation height `u64::MAX`,
scheduled under ADR-082 no sooner than `MIN_UPGRADE_LEAD_BLOCKS`
(`crates/governance/src/limits.rs:90`) after the vote.

## Decision needed from Eric

1. Which option: A (nothing), B (rent), C (deposit), or D (expiry, which
   presumes C)?
2. If B or C: the default `deposit_per_byte` / `rent_per_byte_block` at
   activation, in coins per byte (a number nobody has measured; the ADR
   proposes none).
3. If C: may a plain `Transfer` to a fresh address be refused when the
   sender cannot also cover the deposit? (This is the one change to
   "transfers are never gated".)
4. Who receives rent (B): burn, fee collector, or treasury (ADR-111)?
   Deposits (C) are refundable and go nowhere.
5. Should contract storage be priced by the same rule as accounts, or
   left to fuel until the per-byte write cost is verified?
