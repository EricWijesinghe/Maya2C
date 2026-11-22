# ADR-105: Name service (name to address, renewal by height, anti-squatting, look-alike protection)

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-3, CATALOG row `names`, [prior-art/name-service.md](../prior-art/name-service.md), [ADR-081](ADR-081-address-text-bech32m.md), [ADR-102](ADR-102-stealth-addresses-ml-kem.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 25, 28, 32; Standing Order 4

## Context

Addresses have a Bech32m text form, `maya1…`, `tmaya1…`, `dmaya1…`
(ADR-081; `crates/address/src/lib.rs:18-70`). There is no name record and no
name transaction kind; CATALOG row `names` leaves the choice between a VM
contract and a new state prefix open. ENS rents names by the year with a
grace period, a decaying premium after expiry, commit-reveal registration and
a Unicode normalization standard that still admits look-alikes (prior-art
file). The chain has no wall clock (Standing Order 4), so every period here is
a block count.

## Decision

**Names are native records under a new prefix, rented by block count, with
ASCII-only labels, commit-reveal registration and a decaying post-expiry
premium.**

### Name form

`label.maya` on mainnet (`label.tmaya` on testnet, mirroring ADR-081's HRPs).
A label is 3 to 63 bytes of `a-z`, `0-9`, `-`, no leading or trailing `-`,
no `--`. **ASCII only**: no Unicode, so no cross-script homoglyph can be
registered. The cost is no non-Latin names in v1, stated here rather than
solved badly. A name contains a `.`; a Bech32m address contains none
(`bech32m.rs:7` charset), so a wallet never mistakes one for the other.

Within ASCII the remaining confusables are `0/o`, `1/l`, `rn/m`, `vv/w`.
The chain does not refuse them (a rule list would be arbitrary and
contestable); the wallet runs the existing look-alike guard
(`crates/reference-apps/src/dex_front.rs:7`) over the user's address book and
warns. Payee-check (CATALOG) covers the rest.

### State (invariant 25)

New prefix `nm:` in a new committed layer `StateLayer::Names`, in
`RECORD_LAYERS` and the undo journal.

- `nm:n:` ‖ `blake3(label)` → `Name { label, owner, target: Address,
  stealth: Option<(A, ek)>, expires_at: u64, registered_at: u64 }`.
- `nm:r:` ‖ `address` → `label` hash. The reverse record; **written only by a
  transaction whose sender is `address`**, so no name can claim an address it
  does not control.
- `nm:c:` ‖ `commitment` → `height` written. The commit-reveal step.

### Lifecycle (all heights; `YEAR_BLOCKS = 31,536,000` as in ADR-082's horizon)

| Step | Rule |
|---|---|
| `CommitName { commitment }` | `commitment = blake3(label ‖ sender ‖ salt)`. Stored with its height. |
| `RegisterName { label, salt, periods, target }` | A commitment by this sender must exist, aged `MIN_COMMIT_AGE` (60 blocks) to `MAX_COMMIT_AGE` (86,400 blocks). Name must be free, or expired past its grace. `expires_at = height + periods * YEAR_BLOCKS`, `periods` in `1..=10`. Price per the fee rule below. A block producer who sees the reveal cannot register first: it has no valid aged commitment. |
| `RenewName { label, periods }` | Anyone may pay; `expires_at += periods * YEAR_BLOCKS`, capped at `height + 10 * YEAR_BLOCKS` so a name cannot be locked for a century. |
| Grace | For `GRACE_BLOCKS` (7,776,000, 90 days) after `expires_at` only the owner may renew; the name still resolves. |
| Premium | After grace, anyone may register; the price carries a premium that starts at `PREMIUM_START` and halves every `PREMIUM_HALF_LIFE` blocks until it is below one base unit, then is zero. Computed in integers from the block height. |
| `SetTarget`, `SetStealth`, `SetReverse`, `TransferName` | Owner only (reverse: the target address's owner, see above). |

Resolution never happens in consensus. A transaction names an address, and a
wallet resolves `alice.maya` to `maya1…` by reading `nm:n:` and shows both
before signing. A name is a convenience over the address, not a replacement
for it, so a hijacked name cannot redirect a signed transaction.

### Pricing shape (numbers are Eric's)

`price = base_per_period(len) * periods + premium(height)`, paid as an extra
output to `FEE_SINK` (burned) or to the treasury: the destination is an open
question. `base_per_period` steps by label length (3, 4, 5+), as ENS does,
because short names are the squatting target.

## Alternatives considered

- **VM contract.** Would work, but the wallet and explorer would have to run
  contract code to resolve, and the reverse-record rule could not be enforced
  by the chain's sender check. Rejected for the native record.
- **Unicode with ENSIP-15 normalization.** Admits confusables by design and
  needs a normalization table under consensus that changes with Unicode
  versions. Rejected for v1; a later ADR may add script-restricted labels.
- **Consensus-level confusable refusal (`0/o`, `1/l`).** Arbitrary and
  contestable; moved to the wallet.
- **One-time purchase (Solana-style).** No renewal means no reclaim of
  abandoned names. Rejected.
- **Auction for every new name.** Slower and heavier than commit-reveal plus
  length pricing; the decaying premium already covers the contested case
  (expired names).

## Consequences

- Eight new `TxKind` arms, one layer, three prefixes; `Module::Names` for the
  guard (invariant 28); refused from a vault (invariant 32).
- Wallets gain name resolution and the look-alike warning; the explorer gains
  name pages; the site follows.
- A stealth meta-address can be published under a name (ADR-102).
- Expired names resolve during grace and then stop; apps must handle a
  `None`.

## Invariants touched

- **25**: new prefix `nm:` and layer.
- **28**: `Module::Names`.
- **32**: name kinds are not transfer doors.
- Standing Order 4: every period is a block count; no timestamp.

## Activation

`FeatureSet::NAMES = 1 << 13`, compiled `NAMES_ACTIVATION_HEIGHT = u64::MAX`,
scheduled under ADR-082 with the v4 bundle at **genesis + 15,552,000**.

## Test plan

Red tests, `crates/node/tests/names_tests.rs`:

- `a_reveal_without_an_aged_commitment_is_refused` (front-running)
- `a_label_with_a_non_ascii_byte_is_refused_at_decode`
- `a_reverse_record_is_written_only_by_the_address_owner`
- `only_the_owner_renews_during_grace_and_anyone_after`
- `the_premium_halves_per_half_life_and_reaches_zero`
- `renewal_cannot_push_expiry_past_ten_years`
- `a_reorg_restores_owner_and_expiry`
- `the_root_is_unchanged_when_no_name_exists`

Vectors (`spec/03-state.md`, NM-1..NM-5): record bytes; a commitment and its
reveal; premium values at 0, 1 and 2 half-lives; a refused label set
(`-a`, `a--b`, `ab`, 64 bytes, `é`).

## Open questions for Eric (economics and pricing only)

1. `base_per_period` for 3, 4 and 5+ character labels.
2. `PREMIUM_START` and `PREMIUM_HALF_LIFE`.
3. Burn or treasury for name fees?
4. Reserved labels at activation (brand, `maya2c`, numerals) and who holds them.
