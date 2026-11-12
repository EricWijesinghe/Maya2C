# ADR-135: v4 "own anything, pay any way": one fork for tokens, NFTs, names, sponsored fees, smart-account policy and scheduled payments

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled. Supersedes the activation, feature-bit and wire-version sections of ADR-103, 104, 105, 106, 107 and 109 (PR #465); their designs stand except where this ADR says otherwise.
**Date:** 2026-11-12
**Related:** `.claude/plans/v4-great-upgrade.plan.md` (approved 2026-11-12), parity matrix #466, [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), [ADR-046](ADR-046-universe-scale-supply.md), [ADR-036](ADR-036-chain-bound-signatures.md), [ADR-030](ADR-030-vault-accounts.md), [ADR-072](ADR-072-max-block-bytes.md), ADR-086/PR #447 (account authority, wire v9, bit 9), ADR-102 (stealth, wire v10, bit 10, v3), ADR-114 (contract timers), ADR-117 (multisig v2, wire v12); invariants 7, 17, 24, 25, 28, 29, 31, 32, 33, 34; Standing Orders 2, 4, 9.
**Threat models:** [docs/threat-models/](../threat-models/README.md) `v4-*.md`. **Prior art:** [pq-smart-accounts](../prior-art/pq-smart-accounts.md), [pq-name-service](../prior-art/pq-name-service.md), [pq-native-ownership](../prior-art/pq-native-ownership.md), [native-scheduled-payments](../prior-art/native-scheduled-payments.md).

## Context

Six parity gaps (P-1, P-2, P-3, P-4, P-5, P-7) each have a proposed ADR on
PR #465. Shipped as six forks they would be six activation heights, six
vector sets and six audit surfaces. v4 ships them as **one** coordinated
hard fork with one schedule entry, one vector set and one red-team pass,
while each feature keeps its own bit so any one can be left dark.

What exists today, checked in the tree on 2026-11-12:

- Assets: `TxKind::RegisterAsset`/`TransferAsset` (`crates/node/src/core/payload.rs:458-460`),
  `AssetRecord { creator, total_supply: U256, symbol[8] }` 72 bytes under
  `d:asset:` (`crates/node/src/state/asset.rs:48-66`, `state/dex.rs:51`).
  No mint, by a written decision (`asset.rs:17-25`). No decimals, by design.
- Account authority (PR #447): wire v9, prefix `ka:`, `Module::Account`,
  bit 9, `TAG_ACCOUNT = 54`. Session keys and spending limits are **not**
  in it; `crates/smart-account/src/policy.rs` has them as RESEARCH.
- Fees: an ordinary output to `FEE_COLLECTOR`, `required(size) = base_fee *
  size` with saturation (`crates/node/src/state/fees.rs:92-98`).
- Upgrade schedule: `up:` records, `FeatureSet` bits 0-7 compiled
  (`crates/node/src/upgrade.rs:44-60`), `SUPPORTED_PROTOCOL_VERSION = 2`,
  `MIN_UPGRADE_LEAD_BLOCKS = 2,592,000` (`crates/governance/src/limits.rs:90`),
  `MAX_SCHEDULE_HORIZON_BLOCKS = 31,536,000`.
- Wire versions in the tree: 5, 6 (hybrid), 7 (suite), 8 (multisig).
  Claimed: 9 (#447), 10 (ADR-102), 11 (ADR-106), 12 (ADR-117).
- Prefixes committed today (`rg 'PREFIX: &\[u8\] = b"'`, 2026-11-12):
  `acct:` `chan:` `code:` `cstate:` `null:` (typed), `d:` `g:` `h:` `i:` `k:`
  `m:` `o:` `q:` `r:` `sl:` `t:` `up:` `v:` (record layers), `blk:` `bdelta:`
  `undo:` (local-only). Claimed by open ADRs: `ka:` (#447), `n:` (104),
  `nm:` (105), `sp:` (109), `ct:` (114), `cj:` `ag:` `k:cmc:` (125-130),
  `rent:` `k:opt:` `st:` `ac:` (131-134).

### Findings that contradict the briefs (Standing Order 9)

1. **Three block-time conventions exist.** Invariant 34 and
   `MIN_UPGRADE_LEAD_BLOCKS` say "one block per second";
   `crates/consensus/difficulty.rs:32` says `TARGET_BLOCK_TIME = 15`
   seconds (the PoW target); `crates/smart-account/src/policy.rs:5` says
   `BLOCKS_PER_DAY = 43_200` ("2 s blocks"). Every period in ADR-103..109
   assumed one block per second. This ADR writes every period as a block
   count under the invariant-34 convention and names the constant, so one
   edit fixes it when the rate is settled. **Open item O-1, below.**
2. **ADR-103..109 placed v4 at genesis + 15,552,000.** The v3 rule
   (memory, 2026-11-11) is "v3 activates at least 3 months after v2" and
   v2 is "≥ genesis + 3 months"; v4 must be ≥ v3 + 3 months. No v2 or v3
   umbrella ADR is on `origin/master`, `postgen/v2` or `postgen/v3` yet
   (ADR-095 is claimed, not written). The height in #465 is superseded.
3. **ADR-103's "trailing optional section" on `RegisterAsset` is
   ambiguous to decode.** `TxKind` is the last section of a frame and
   `reader.finish()` demands nothing follow it, so "a v1-shaped
   registration remains valid" means a decoder that chooses by remaining
   length. Replaced by a new tag (`RegisterCappedAsset`), below.
4. **ADR-104's `SellItem` is the one multiplication in the fork** and
   its royalty is not enforceable off chain. Cut to v5 (see "Cut list").

## Decision

**v4 is one schedule entry that switches on six feature bits, 41-46. Each
feature is a `TxKind` family or one wire version, each has a compiled
`u64::MAX` fallback and its own invariant-guard module, and every amount is
`U256` with checked arithmetic and a written bound — no 512-bit
intermediate anywhere.**

### Fork contents

| # | Feature | Bit | Wire | `TxKind` tags | Prefix / layer | Module (inv 28) | Source ADR |
|---|---|---|---|---|---|---|---|
| P-1 | Token mint/burn | 41 `ASSET_MINT` | v6-v9 payload | 55 `RegisterCappedAsset`, 56 `MintAsset`, 57 `BurnAsset`, 58 `RenounceMint` | `d:asset:` record v2 (no new prefix) | `Module::Dex` | 103 |
| P-2 | Unique assets | 42 `NFT` | v6-v9 payload | 59 `CreateCollection`, 60 `MintItem`, 61 `TransferItem`, 62 `BurnItem` | `n:` / `StateLayer::Nft` | `Module::Nft` (new) | 104 (sale cut) |
| P-3 | Names | 43 `NAMES` | v6-v9 payload | 63 `CommitName`, 64 `RegisterName`, 65 `RenewName`, 66 `UpdateName` | `nm:` / `StateLayer::Names` | `Module::Names` (new) | 105 (premium cut) |
| P-4 | Sponsored fees | 44 `SPONSORED_FEES` | **v13** outer frame | none | none | none (a fee rule) | 106 (v11 → v13) |
| P-5 | Smart-account policy | 45 `ACCOUNT_POLICY` | v9 (#447) | sub-ops of 54 `Account`: `AddSessionKey`, `RevokeSessionKey`, `SetDailyLimit` | `ka:s:`, `ka:w:` under `ka:` | `Module::Account` (#447) | 107 (allow-list, delay cut) |
| P-7 | Scheduled payments | 46 `SCHEDULED_PAYMENTS` | v6-v9 payload | 67 `CreateSchedule`, 68 `CancelSchedule` | `sp:` / `StateLayer::Scheduled` | `Module::Scheduled` (new) | 109 (top-up cut) |

Tags 55-68 are assigned here after `TAG_ACCOUNT = 54` (#447). A collision
with a tag another branch lands first is renumbered by the coordinator, as
ADR numbers are.

### Activation and kill switches

- **One entry.** `ScheduleRequest { activation: V4_ACTIVATION, version:
  Some(V4_VERSION), features: bits 41..=46, sunsets: [] }` under ADR-082.
  `V4_VERSION` is the next rules epoch after v3's (assigned at
  implementation; `SUPPORTED_PROTOCOL_VERSION` rises with it).
- **Height.** `V4_ACTIVATION >= V3_ACTIVATION + QUARTER_BLOCKS`, where
  `QUARTER_BLOCKS = 7_776_000` (90 days under the invariant-34 rate). With
  v2 ≥ genesis + `QUARTER_BLOCKS` and v3 ≥ v2 + `QUARTER_BLOCKS`, the
  floor is **genesis + 23,328,000** (270 days). Pinned by a test in the
  style of `upgrade_schedule_tests.rs` that refuses a v4 entry earlier
  than v3 + `QUARTER_BLOCKS`. The horizon (`MAX_SCHEDULE_HORIZON_BLOCKS`,
  one year) bounds the other side, so v4 is scheduled from v3's activation
  onward, not from genesis.
- **Compiled fallback.** Each of the six has `*_ACTIVATION_HEIGHT =
  u64::MAX` in `crates/node/src/state/upgrades.rs`, resolved from the
  schedule like bits 0-7. A feature not named in the entry stays dark on
  every node identically (ADR-002: a height, never a cargo feature).
- **Pre-activation kill.** Leave the bit out of the entry. Nothing else
  changes; the vectors for that feature become "refused before
  activation" vectors.
- **Post-activation kill.** The invariant-guard breaker on the feature's
  module (invariant 28): `Module::Nft`, `Module::Names`, `Module::Scheduled`
  halt their kinds and, for `Scheduled`, skip the end-of-block step (due
  entries carry over). `ASSET_MINT` kinds sit under `Module::Dex`, so a
  breaker there halts minting **and** trading; accepted, because a mint
  bug and a pool bug both need trading stopped. `ACCOUNT_POLICY` sits
  under `Module::Account`. Sponsored fees have no module: a sponsorship
  bug is a fee-rule bug, and the mempool admission flag
  `accept_sponsored` (node-local, non-consensus) stops *relaying* v13
  frames while a breaker is not needed for state.
- Before activation every v4 kind is refused on admission and on apply
  (both call the same check, the property invariant 31 protects), and a
  v13 frame is refused at decode with the version named.

### Encoding conventions (all forms)

As `crates/node/src/core/codec.rs`: `Address` 32 bytes; `U256` 32 bytes
little-endian (`put_amount`); `u64`/`u32` little-endian; variable sections
are `u32 LE len ‖ bytes`. **State keys** use big-endian integers so RocksDB
key order is numeric order (ADR-104/109 convention). A `TxKind` payload is
`tag(1) ‖ fields`; the frame is unchanged (v6 hybrid, v7 suite, v8
multisig, v9 account-authority all carry a `TxKind`).

### P-1 Token mint and burn (bit 41)

**Record v2** at `d:asset:` ‖ `asset_id(32)`:

```
v1 (today, 72 B): creator(32) ‖ total_supply(32 LE) ‖ symbol(8)
v2 (136 B):       0x02 ‖ creator(32) ‖ total_supply(32 LE) ‖ symbol(8) ‖ cap(32 LE) ‖ authority(32)
```

Decode by length: 72 → v1 with `cap = total_supply`, `authority = [0; 32]`;
136 with leading `0x02` → v2; anything else is a decode error. A v1 record
is rewritten as v2 only when an operation touches it. `authority == [0; 32]`
means none (`NATIVE_ASSET` owns nothing, `asset.rs:14-15`). Leaf:
`blake3("maya2c asset leaf v2" ‖ key ‖ record)` for v2; v1 leaves unchanged,
so a chain that never mints keeps its root.

**Tx forms** (`tag ‖ fields`, sizes in bytes):

| Tag | Form | Layout | Size |
|---|---|---|---|
| 55 | `RegisterCappedAsset` | `symbol(8) ‖ total_supply(32) ‖ cap(32) ‖ authority(32)` | 105 |
| 56 | `MintAsset` | `asset(32) ‖ to(32) ‖ amount(32)` | 97 |
| 57 | `BurnAsset` | `asset(32) ‖ amount(32)` | 65 |
| 58 | `RenounceMint` | `asset(32)` | 33 |

`RegisterAsset` (tag unchanged) stays the fixed-supply form and keeps
writing a 72-byte v1 record.

**Rules.** `RegisterCappedAsset`: `validate_symbol`; `total_supply <= cap`;
`authority ∈ {sender, [0;32]}`; whole `total_supply` to the sender; a cap
with no authority is accepted (it is a v1 asset with a documented ceiling).
`MintAsset`: sender == authority ≠ none; `total_supply.checked_add(amount)`
exists and `<= cap`; `amount > 0`; credit `to` (`checked_add` on the
balance). `BurnAsset`: holder's balance `checked_sub(amount)`;
`total_supply.checked_sub(amount)`; cap unchanged. `RenounceMint`: sender ==
authority; `authority = none`, `cap = total_supply`. Every failure is a
refusal (not a no-op, not a nonce burn: refused at admission and at apply by
the same function). The asset id derivation (`derive_asset_id`, creator ‖
nonce ‖ symbol) is unchanged, so a capped asset and a fixed asset cannot
share an id.

**Money math.** Four `checked_add`/`checked_sub` comparisons against `cap`.
No multiplication.

**Vault (inv 32):** all four kinds are non-transfer doors, refused from a
vault like `TransferAsset` today.

### P-2 Unique assets (bit 42)

**State** under `n:` (`StateLayer::Nft`, added to `RECORD_LAYERS`):

```
n:c: ‖ collection_id(32)                 → 0x01 ‖ creator(32) ‖ max_items(8 LE) ‖ minted(8 LE) ‖ metadata_hash(32)   (81 B)
n:i: ‖ collection_id(32) ‖ index(8 BE)   → 0x01 ‖ owner(32) ‖ metadata_hash(32)                                       (65 B)
n:o: ‖ owner(32) ‖ collection_id(32) ‖ index(8 BE) → (empty)       the owner index, prefix-scanned by wallets
collection_id = blake3::derive_key("maya2c nft collection v1", creator ‖ nonce(8 LE))
```

**Tx forms:**

| Tag | Form | Layout | Size |
|---|---|---|---|
| 59 | `CreateCollection` | `max_items(8) ‖ metadata_hash(32)` | 41 |
| 60 | `MintItem` | `collection(32) ‖ metadata_hash(32)` | 65 |
| 61 | `TransferItem` | `collection(32) ‖ index(8) ‖ to(32)` | 73 |
| 62 | `BurnItem` | `collection(32) ‖ index(8)` | 41 |

**Rules.** `CreateCollection`: `1 <= max_items <= MAX_COLLECTION_ITEMS`
(`2^32`, so `index` fits a `u32` in wallets); creator = sender. `MintItem`:
sender == creator, `minted < max_items`, `index = minted`, `minted += 1`
(a `u64` that cannot overflow under the cap), owner = creator. `TransferItem`:
sender == owner; rewrites `n:i:` and moves the `n:o:` row. `BurnItem`: sender
== owner; deletes `n:i:` and `n:o:`; `minted` does not fall, so an index is
never reused. Every failure is a refusal.

**Hash only.** 32 bytes per item, 32 per collection, nothing else; where the
metadata lives is the app's business (ADR-104). A `TxKind` here is under
100 bytes; invariant 33 is untouched.

**Money math.** None. The royalty multiplication of ADR-104 leaves with the
sale (cut list, C-1).

**Vault (inv 32):** non-transfer doors.

### P-3 Names (bit 43)

**Label**: 3-63 bytes of `a-z0-9-`, no leading/trailing `-`, no `--`
(ADR-105; ASCII only, so no cross-script homoglyph can exist in a record).
`label.maya` / `label.tmaya` / `label.dmaya` follow ADR-081's HRPs. A name
contains `.`; a Bech32m address never does.

**State** under `nm:` (`StateLayer::Names`):

```
nm:n: ‖ blake3(label)(32)  → 0x01 ‖ len(1) ‖ label(len) ‖ owner(32) ‖ target(32) ‖ registered_at(8 LE) ‖ expires_at(8 LE) ‖ stealth_flag(1) [‖ stealth meta-address, ADR-102 encoding]
nm:r: ‖ address(32)        → blake3(label)(32)                     reverse; written only by a tx whose sender is `address`
nm:c: ‖ commitment(32)     → height(8 LE) ‖ committer(32)
commitment = blake3("maya2c name commit v1" ‖ label ‖ sender ‖ salt(32))
```

**Tx forms:**

| Tag | Form | Layout | Size |
|---|---|---|---|
| 63 | `CommitName` | `commitment(32)` | 33 |
| 64 | `RegisterName` | `len(1) ‖ label(len) ‖ salt(32) ‖ periods(1) ‖ target(32)` | 70-130 |
| 65 | `RenewName` | `len(1) ‖ label(len) ‖ periods(1)` | 6-66 |
| 66 | `UpdateName` | `len(1) ‖ label(len) ‖ op(1) ‖ arg` with `op`: `0x01 SetTarget(address 32)`, `0x02 SetStealth(flag 1 [‖ meta-address])`, `0x03 SetReverse(none; sender's address)`, `0x04 Transfer(new_owner 32)`, `0x05 ClearStealth(none)` | 6-~1,700 |

**Lifecycle** (block counts; `YEAR_BLOCKS = 31_536_000`, `GRACE_BLOCKS =
7_776_000`, `MIN_COMMIT_AGE = 60`, `MAX_COMMIT_AGE = 86_400`, all under the
invariant-34 rate; O-1 applies):

- `CommitName`: writes `nm:c:` with the height and sender. Re-committing
  the same commitment is refused (the age must not reset). A commitment
  older than `MAX_COMMIT_AGE` is garbage: `RegisterName` deletes the one
  it consumes; stale ones are pruned by the registering transaction of the
  same label only (no sweep; bounded work).
- `RegisterName`: `commitment` recomputed from `(label, sender, salt)` must
  exist, with `committer == sender` and `MIN_COMMIT_AGE <= height - h_c <=
  MAX_COMMIT_AGE`; the label is free (no record, or `height > expires_at +
  GRACE_BLOCKS`); `1 <= periods <= 10`; `expires_at = height + periods *
  YEAR_BLOCKS`; price = `base(len) * periods` to `FEE_SINK` or treasury
  (**O-3**). A block producer that sees the reveal has no aged commitment of
  its own and cannot register first; a producer that drops the reveal only
  delays it (the commitment stays valid to `MAX_COMMIT_AGE`).
- `RenewName`: anyone pays; `expires_at = min(expires_at + periods *
  YEAR_BLOCKS, height + 10 * YEAR_BLOCKS)`. During grace, owner only.
- After grace: free at the base price. The decaying premium of ADR-105 is
  cut (C-2).
- `UpdateName`: owner only, except `SetReverse`, which anyone may call for
  **their own** address and which requires `target == sender` at that
  moment (so a name can only reverse-resolve to an address that points at
  it). `Transfer` clears the reverse record of the old owner's address if
  it names this label.
- Resolution never happens in consensus: a transaction names an address;
  the wallet shows both (ADR-105).

**Money math.** `base(len)` is a governed `U256` parameter (one per length
class 3, 4, 5+; invariant 17); `periods <= 10 < 2^4`, so `base(len) *
periods` fits whenever `base(len) < 2^252`, which the parameter admission
refuses above. One `checked_mul` with that bound written beside it, then a
`checked_add` for the fee output. No division.

**Vault (inv 32):** non-transfer doors; the price output is to `FEE_SINK`,
which is not the collector, so it counts toward the vault window like any
output.

### P-4 Sponsored fees (bit 44, wire v13)

ADR-106 unchanged except the version byte: **`0x0D`** (v13; 11 was claimed
before ADR-117 took 12 and the plan reserved 13 for v4).

```
0x0D ‖ inner_len(4 LE) ‖ inner (one v6/v7/v8/v9 frame; an inner v13 or v10+ stealth-wrapped v13 is refused: depth 1)
     ‖ suite(1) ‖ pk_len(4 LE) ‖ sponsor_pk ‖ max_fee(32 LE) ‖ valid_until(8 LE) ‖ sig_len(4 LE) ‖ signature
sponsor signing bytes = "maya2c/sponsor/v1" ‖ chain_tag(32) ‖ txid(inner)(32) ‖ max_fee(32 LE) ‖ valid_until(8 LE)
```

**Rules** (ADR-106 R1-R6, restated as the consensus list): inner has no
output to `FEE_COLLECTOR`; `required(outer_len) <= max_fee`; `height <=
valid_until`; `valid_until - height <= MAX_SPONSOR_WINDOW` (86,400) at
admission; sponsor suite admissible at `height` (inv 29/31); sponsor address
= `suite_address(suite, pk)`; at apply, `sponsor.balance.checked_sub(required
(outer_len))` to `FEE_COLLECTOR` **then** the inner; failure of either refuses
the whole frame and consumes no nonce. Base-fee burn and tip split unchanged.
`verify` refuses v13; `verify_at` verifies inner then sponsorship.

**Mempool** (node-local, must not differ between admission and apply for
*validity*, may differ for *ordering*): `MAX_SPONSORED_PENDING_PER_SPONSOR =
64`; pending `Σ max_fee` per sponsor `<= sponsor balance` (newest dropped);
sponsored frames evict before unsponsored at equal tip. These three are
admission policy, never consensus; a block that holds a 65th frame from one
sponsor is valid if every frame applies.

**Money math.** `required()` already saturates; one `checked_sub` on the
sponsor balance; `Σ max_fee` in the mempool is `checked_add` with saturation
(an overflowing sum is "cannot cover").

**Vault (inv 32):** the sponsor's debit is an output to the collector and
counts toward the window above the 4× rule, as today.

### P-5 Smart-account policy (bit 45, on wire v9)

Session keys and a daily window, as chain-evaluated rules on PR #447's v9
frame, through one state-aware entry point
`Transaction::verify_with_state(height, policy, chain, &dyn AuthorityReader)`
that the mempool and block execution both call (ADR-107). `verify` and
`verify_at` keep refusing v9 (invariant 31).

**State** under `ka:` (existing layer):

```
ka:s: ‖ account(32) ‖ key_hash(32)      → 0x01 ‖ max_amount(32 LE) ‖ expires_at(8 LE) ‖ n(4 LE) ‖ recipients(32 × n)    n <= 16
ka:w: ‖ account(32) ‖ window_start(8 BE) → spent(32 LE)
ka:a: (…#447 record…) gains daily_limit(32 LE) as a trailing field of its next record version
```

`window_start = height - (height % BLOCKS_PER_DAY)`, `BLOCKS_PER_DAY =
43_200` as the compiled constant the smart-account crate already names
(O-1 applies).

**Account sub-ops** (inside `TAG_ACCOUNT = 54`'s action byte, numbers after
#447's last): `AddSessionKey { key_hash(32), scope }`, `RevokeSessionKey {
key_hash(32) }`, `SetDailyLimit { limit(32) }` (`0` = unlimited). Each is an
owner-threshold action, never signable by a session key.

**Rules at verification.** A signature by a session key counts only if:
`kind == Transfer`; every output recipient ∈ `recipients`; every output
`amount <= max_amount`; `height <= expires_at`; the frame carries no other
action. The daily window: `spent(window).checked_add(Σ outputs) <=
daily_limit` when `daily_limit != 0`, evaluated for **every** v9 frame
(owner-signed too), and `ka:w:` is written at apply. `verify_cost(suite)`
per signature is charged against `MAX_VALIDATION_COST = 40` before any
signature is checked. A frame that fails any rule is refused; nothing is
written; the nonce is not consumed.

**Expiry.** An expired `ka:s:` is deleted by the next `RevokeSessionKey` or
by any owner-signed frame from that account touching it; a sweep is never
run. An expired key cannot sign (the height check), so the stale row is a
storage cost only, paid by the account that created it.

**Money math.** One `checked_add` and a comparison per frame.

**Vault (inv 32):** an address that is both a vault and a policy account
satisfies both; the stricter wins by construction (ADR-107).

### P-7 Scheduled payments (bit 46)

**State** under `sp:` (`StateLayer::Scheduled`):

```
sp:s: ‖ schedule_id(32)                 → 0x01 ‖ payer(32) ‖ payee(32) ‖ amount(32 LE) ‖ next_due(8 LE) ‖ interval(8 LE) ‖ remaining(4 LE) ‖ escrow(32 LE) ‖ missed(1)   (150 B)
sp:d: ‖ next_due(8 BE) ‖ schedule_id(32) → (empty)       the due index
schedule_id = blake3::derive_key("maya2c schedule v1", payer ‖ nonce(8 LE))
```

**Tx forms:**

| Tag | Form | Layout | Size |
|---|---|---|---|
| 67 | `CreateSchedule` | `payee(32) ‖ amount(32) ‖ first_due(8) ‖ interval(8) ‖ runs(4)` | 85 |
| 68 | `CancelSchedule` | `schedule_id(32)` | 33 |

**Rules.** `CreateSchedule`: `first_due > height`; `MIN_INTERVAL (3_600) <=
interval`; `1 <= runs <= MAX_RUNS (120)`; `escrow = (amount + RUN_FEE)
.checked_mul(runs)` debited now; writes both records. `CancelSchedule`:
payer only; unspent escrow returns; both records deleted. `TopUpSchedule`
is cut (C-4: cancel and recreate is one transaction more and one arm less).

**End-of-block step** in `apply_block_journaled`, after transactions and
**before** ADR-114's timers (a fixed order), scanning `sp:d:` from the start
while `next_due <= height`, at most `MAX_DUE_PER_BLOCK = 256`:

1. `escrow >= amount + RUN_FEE`: credit payee `amount`; `RUN_FEE` to
   `FEE_COLLECTOR`; `escrow -= amount + RUN_FEE`; `remaining -= 1`;
   `missed = 0`.
2. Else `missed += 1`; nothing moves (invariant 7: a no-op, never a block
   failure).
3. `remaining == 0 || missed >= MAX_MISSED (3)`: delete both records, return
   escrow to payer. Else delete the `sp:d:` row and write it at `next_due +
   interval`.

The 257th due entry runs first in the next block (lowest `next_due`). The
step is under the undo journal and the declared `state_root` must reproduce
it (invariant 24); it reads committed state and the height only. Worst-case
work per block: 256 record rewrites and 512 balance credits, stated as the
bound.

**Money math.** `(amount + RUN_FEE)` is a `checked_add`; `.checked_mul(runs)`
with `runs <= 120 < 2^7` fits whenever the sum `< 2^249`, refused above with
the bound written beside the check. Everything else is `checked_add`/
`checked_sub`. `RUN_FEE` is a governed parameter (invariant 17), read from
state, never a `const`.

**Vault (inv 32):** the escrow debit at creation is the vault-limited
movement; runs move escrow the chain already holds.

### Fee and gas per form

Every v4 transaction pays the ordinary byte fee `required(size)` as an
output to `FEE_COLLECTOR` (or through a v13 sponsor). On top, each form has
one named surcharge parameter whose **value is Eric's** (invariant 17: a
governed `U256`, admitted below `2^252`, defaulting to zero at activation
unless he sets it):

| Form | Surcharge parameter | Paid to | Why it exists |
|---|---|---|---|
| `RegisterCappedAsset`, `RegisterAsset` | `AssetRegistrationFee` | `FEE_SINK` (burn) or treasury (O-2) | anti-spam on a permanent 136-byte record |
| `MintAsset` | none | | bytes only; the cap bounds dilution, a fee does not |
| `CreateCollection` | `CollectionCreationFee` | same as above | permanent record |
| `MintItem` | none | | bytes; `max_items` bounds it |
| `RegisterName`, `RenewName` | `NameBaseFee{3,4,5+}` × periods | `FEE_SINK` or treasury (O-3) | the squatting cost |
| v13 sponsorship | `SponsorSurcharge` (O-4) | `FEE_COLLECTOR` | extra admission work (two verifications) |
| v9 policy frame | none in v4 | | `MAX_VALIDATION_COST` bounds the work; a per-signature price is O-5 |
| `CreateSchedule` run | `RUN_FEE` per run, prepaid | `FEE_COLLECTOR` (joins tips) | pays the state write at run time |

There is no gas: none of these forms runs contract code. The byte fee uses
the saturating `required()`; every surcharge is one `checked_add` to the
fee output, refused on overflow.

### Invariant-25 check (prefixes)

`is_committed()` is `starts_with`, so a new prefix collides if it is a
prefix of, or has as a prefix, an existing one. Checked against the table in
Context (2026-11-12):

| New | Against | Result |
|---|---|---|
| `n:` | `null:` (typed), `nm:` (this ADR) | `null:` does not start with `n:` (second byte `u` ≠ `:`); `nm:` likewise. Clear. |
| `nm:` | `n:`, `null:` | `nm:` does not start with `n:`. Clear. |
| `sp:` | `sl:` (stateless), `st:` (ADR-133) | Distinct second byte. Clear. |
| `ka:s:`, `ka:w:` | `ka:a:` (#447), `k:` (staking) | `ka:` does not start with `k:` (second byte `a`). #447 owns `ka:`; the two sub-prefixes are new. Clear. |
| `d:asset:` v2 | `d:asset:` v1 | Same prefix, record versioned by length and leading byte; leaves for v1 unchanged. Clear. |

Each of `n:`, `nm:`, `sp:` is added to `RECORD_LAYERS` with its layer, so
`uncovered_keys()` and `a_new_top_level_prefix_is_caught_until_it_gets_a_layer`
cover it, and every write is an overlay record under `capture_undo`. Three
new `StateLayer` variants are appended after the existing seventeen (and
after #447's `AccountAuthority`); the state-proof layer tag is the variant's
position, appended never reordered. A node that never sees a v4 record keeps
its pre-v4 root: no layer contributes to the fold when it has no records
(the property `the_root_is_unchanged_when_no_*_exists` tests pin for every
existing layer).

### What is NOT in this ADR

- No change to `Transaction::verify` beyond "refuse v13" (invariant 31).
- No change to block selection (`first_fit`, invariant 33): every v4
  transaction is bytes like any other; the end-of-block step adds no bytes.
- No new dependency in any core crate.
- No new suite; every v4 signature is a registry suite under
  `check_admissible` (invariant 29). Session keys are suite keys, not a new
  key type.

## Cut list (recommended for v5)

| # | Cut | Why |
|---|---|---|
| C-1 | ADR-104 `SellItem` + royalties | The sole multiplication in the fork; the royalty is unenforceable against a zero-price sale or an off-chain payment (ADR-104 says so); the co-signed sale needs either a v8 two-party frame or an offer record with expiry and undo. A sale in v4 is a `TransferItem` beside a transfer, or a DvP swap once ADR-061's consent model is extended to `n:` items — a v5 ADR of its own. |
| C-2 | ADR-105 decaying post-expiry premium | Two more numbers nobody has (`PREMIUM_START`, half-life), a halving curve under consensus and three vectors, for the contested-expiry case alone. Grace + base price covers reclaim; a premium can be added without a wire change (it is a fee rule). |
| C-3 | ADR-107 allow-list and large-transfer delay | Vaults already do both (invariant 32, `Policy.delay_above`/`delay_blocks` map onto a vault). Promoting them twice gives two code paths for one rule; ADR-107 itself defers the merge. Session scope + daily window is what P-5 needs. |
| C-4 | ADR-109 `TopUpSchedule` | Cancel + recreate does it in two transactions; one arm and one mutation path less. |
| C-5 | ADR-108 liquid staking | Already outside v4 in the plan; stated so it is not assumed in. |

Not cut, considered: **P-7 scheduled payments** is the heaviest item (the
end-of-block step is the one apply-path change). Kept because ADR-114's
timers will need the same step and the same bound, and because it is the
form users notice (subscriptions, payroll). If the build lane needs to drop
one feature to make the window, drop P-7 first: its bit simply stays out of
the entry.

## Conformance-vector outline

Vectors live in `spec/tests/*.json` and are counted by `cargo xtask
spec-coverage`. Every form gets the same five, plus the listed extras.
"Bound" means the exact accept/refuse pair at the stated limit.

| Form | Encode/decode | Signing bytes | State leaf(s) | Bound accept/refuse | Pre-activation refusal | Extras |
|---|---|---|---|---|---|---|
| `RegisterCappedAsset` 55 | yes | yes (v6 and v7 frame) | v2 record 136 B | `total_supply == cap` / `cap + 1` | yes | v1 72 B record decodes as capped-at-supply, no authority |
| `MintAsset` 56 | yes | yes | record after mint | `supply + amount == cap` / `+1` | yes | non-authority refused; `amount = 0` refused |
| `BurnAsset` 57 | yes | yes | record after burn | `amount == balance` / `+1` | yes | cap unchanged after burn |
| `RenounceMint` 58 | yes | yes | record with `cap = supply`, authority none | n/a | yes | second renounce refused |
| `CreateCollection` 59 | yes | yes | `n:c:` 81 B | `max_items = 1` / `0`; `= 2^32` / `+1` | yes | `collection_id` derivation vector |
| `MintItem` 60 | yes | yes | `n:i:`, `n:o:` | `minted == max_items - 1` / `== max_items` | yes | non-creator refused |
| `TransferItem` 61 | yes | yes | both rows moved | n/a | yes | non-owner refused |
| `BurnItem` 62 | yes | yes | rows deleted, `minted` unchanged | n/a | yes | re-mint after burn gets a new index |
| `CommitName` 63 | yes | yes | `nm:c:` | n/a | yes | duplicate commitment refused |
| `RegisterName` 64 | yes | yes | `nm:n:`, consumed `nm:c:` | age `MIN_COMMIT_AGE` / `-1`; `MAX_COMMIT_AGE` / `+1`; `periods 10` / `11` | yes | refused labels `-a`, `a--b`, `ab`, 64 B, `é`; wrong committer; price at `base × periods` |
| `RenewName` 65 | yes | yes | `expires_at` | `+10 years` cap / over | yes | non-owner during grace refused, after grace accepted |
| `UpdateName` 66 | yes (each op) | yes | record per op | n/a | yes | `SetReverse` with `target ≠ sender` refused; `Transfer` clears old reverse |
| v13 sponsorship | outer bytes for v6, v7 and v9 inner | sponsor bytes | n/a | `required(outer) == max_fee` / `max_fee - 1`; `height == valid_until` / `+1` | decode refusal naming v13 | wrong chain tag; inner with fee output; inner v13 (depth); sponsor short by 1 |
| v9 `AddSessionKey` / `RevokeSessionKey` / `SetDailyLimit` | yes | yes (owner-signed) | `ka:s:`, `ka:a:` | `n = 16` / `17` recipients | yes | session-key-signed sub-op refused |
| v9 session-scoped transfer | yes | yes (session key) | `ka:w:` | `amount == max_amount` / `+1`; `height == expires_at` / `+1`; `spent + Σ == daily_limit` / `+1` | yes | unlisted recipient; `kind ≠ Transfer`; validation cost at 40 / 41 |
| `CreateSchedule` 67 | yes | yes | `sp:s:`, `sp:d:` | `runs 120` / `121`; `interval 3600` / `3599`; `first_due = height + 1` / `= height` | yes | escrow arithmetic at `2^249 - 1` / `2^249` |
| `CancelSchedule` 68 | yes | yes | rows deleted, balance | n/a | yes | non-payer refused |
| end-of-block step | n/a | n/a | state after each due height for a 2-run schedule | 256 due / 257 (carry-over) | step skipped | miss sequence 1, 2, 3 (closed); root with and without a due run |

Vector id prefixes: `AS-` tokens, `NF-` NFTs, `NM-` names, `SP-` sponsored,
`AA-` (continuing #447's) policy, `SC-` schedules, `V4-` activation (the
entry at `v3 + QUARTER_BLOCKS` accepted, `- 1` refused; each bit left out
stays dark; the six `u64::MAX` fallbacks).

## Security-architect pass (adversarial read of this ADR, 2026-11-12)

Holes found in the first draft and the fix applied in the text above:

| # | Hole | Fix |
|---|---|---|
| S-1 | A session key could sign `AddSessionKey` for itself with a wider scope (privilege escalation). | Account sub-ops are owner-threshold actions, never signable by a session key; vector "session-key-signed sub-op refused". |
| S-2 | `SetReverse` by the address owner could point an address at a name whose `target` is someone else, so a wallet showing "reverse name" would attribute a name the owner does not control. | `SetReverse` requires `target == sender`; `Transfer` clears the old reverse. |
| S-3 | `CommitName` re-submission reset the age, letting a committer keep a commitment perpetually fresh and grind it against reveals. | Duplicate commitment refused; age never resets. |
| S-4 | Stale `nm:c:` rows accumulate forever (a free permanent write per commit). | Pruned by the registering transaction of the same label; the `CommitName` byte fee is the storage price; O-3 may add a commit fee. |
| S-5 | Mempool rules for sponsors (64 pending, Σ max_fee) were written as if consensus; a block carrying a 65th frame would be refused by some nodes. | Stated as admission policy only; validity is per-frame apply. |
| S-6 | Mint `amount = 0` was accepted: a free way to rewrite the record to v2 and churn the root. | Refused. |
| S-7 | `BurnItem` then re-mint could reuse an index and resurrect a burned item's owner row. | `minted` never falls; indices are never reused. |
| S-8 | A `u64::MAX` fallback for `SCHEDULED_PAYMENTS` with a breaker on `Module::Scheduled` left due rows growing without bound during a halt. | Accepted: rows are prepaid; carry-over resumes in order; no sweep. Documented as the bound. |
| S-9 | Daily window checked only for session-key frames let an owner-signed frame and a session frame straddle the limit. | Window evaluated for every v9 frame of a limited account. |
| S-10 | `ACCOUNT_POLICY` bit could activate before `ACCOUNT_AUTHORITY` bit 9. | Entry admission refuses bit 45 unless bit 9 is active at or before `V4_ACTIVATION`; vector under `V4-`. |
| S-11 | `n:` vs `null:` collision was asserted, not checked. | Byte-level `starts_with` check written in the invariant-25 table. |
| S-12 | Three block-time conventions make every "day" in this ADR up to 15× off. | Every period names its constant; O-1 is the blocking question before any period is used in a vector. |

## Consequences

- One schedule entry, one vector set (`V4-` plus six families), one
  activation rehearsal (plan V2), one red-team pass (plan V1).
- Fourteen new `TxKind` tags (55-68), three account sub-ops, one wire
  version (13), three state layers, two record kinds under `ka:`, one
  record version under `d:asset:`, one end-of-block step, six feature
  bits (41-46), three guard modules.
- Every caller of `verify_at` in the mempool and apply path moves to
  `verify_with_state` in one change (invariant 31's one-change rule).
- `features.toml`: `smart-account` moves RESEARCH → REAL `working` when its
  tests pass; new ids `asset-mint`, `nft`, `names`, `sponsored-fees`,
  `scheduled-payments`, each `planned` on this PR. Site pages follow each
  build PR (feedback: site follows every upgrade).
- ADR-103..109 keep their design text; their activation, bit and wire
  sections are superseded here. `adr-claims.txt` gains the line for bits
  41-46 and wire v13; bits 11-17 and wire v11 from the #465 claim are
  released.

## Open questions for Eric

Economics and the one timing fact. None blocks D1/D2; O-1 blocks any
vector that contains a period.

| # | Question | Default if unanswered |
|---|---|---|
| O-1 | **Block rate.** Invariant 34 and governance limits assume 1 block/s; the PoW target is 15 s; smart-account says 2 s. Which is the mainnet rate the day-counts in this ADR convert at? | Keep the invariant-34 convention (1/s) and name every constant; re-derive in one edit when answered. |
| O-2 | **Issuance.** `AssetRegistrationFee` and `CollectionCreationFee`: how much, and burn (`FEE_SINK`) or treasury? Any surcharge on `MintAsset` beyond bytes? | Zero surcharge; bytes only; burn. |
| O-3 | **Names.** `NameBaseFee` for 3, 4 and 5+ characters per year; burn or treasury; a fee on `CommitName`; reserved labels at activation (`maya2c`, brand, numerals) and who holds them. | Zero; burn; no commit fee; no reserved labels. |
| O-4 | **Paymaster.** Sponsor-only (as written), or a protocol surcharge `SponsorSurcharge` on every v13 frame? Are 64 pending / 86,400-block window acceptable node defaults? | Sponsor-only, zero surcharge; defaults as written. |
| O-5 | **Smart accounts.** A per-signature price for v9 policy frames (up to 40 cost units of verification), or bytes only? | Bytes only. |
| O-6 | **Schedules.** `RUN_FEE` at activation; burn or join tips; `MAX_RUNS = 120`, `MIN_INTERVAL = 3,600` as governed parameters or compiled? | Zero `RUN_FEE` at activation (governed, raisable); joins tips; compiled. |
| O-7 | **Scope.** Confirm the fork contents table and the cut list C-1..C-5. If one feature must go to make the window, P-7 goes first. | As written. |

### Eric's answers (2026-11-12)

| # | Answer |
|---|---|
| O-1 | 1 block/s (block-time fix A: governance windows re-derived at 1 s, genesis pins the 500 ms round interval). |
| O-2 | Bytes only, no registration or mint surcharge; any fee goes to `FEE_SINK` (burn). |
| O-3 | **Changed from the default.** `NameBaseFee` is non-zero for 3- and 4-character labels and near zero for 5+, per year, burned; values are governed parameters proposed in the I3 PR, not fixed here. No fee on `CommitName`. One reserved label, `maya2c`, held by a governance proposal, not by a key. Reason: a zero fee lets one account register every short label at activation. |
| O-4 | Sponsor-only, zero surcharge; 64 pending / 86,400-block window as node defaults. |
| O-5 | Bytes only. |
| O-6 | `RUN_FEE` zero at activation (governed, raisable), joins tips; `MAX_RUNS` and `MIN_INTERVAL` compiled. |
| O-7 | All six features as written; if the window slips, P-7 (schedules) moves to v5 first. |
