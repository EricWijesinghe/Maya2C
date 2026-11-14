# ADR-110: Shielded transfers of user-issued tokens in the existing pool

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-20, [prior-art/shielded-tokens.md](../prior-art/shielded-tokens.md), [ADR-103](ADR-103-native-fungible-tokens.md), [ADR-037](ADR-037-mainnet-shielded-off.md), [ADR-067](ADR-067-shielded-and-private-amounts-256-bit.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 25, 28, 31, 32, 33; Standing Order 4; Eric's 256-bit rule (2026-11-01)

## Context

The shielded pool is native-coin only. `ShieldedPool { tree, anchors, balance:
U256 }` (`crates/node/src/state/shielded.rs:55-67`) holds one balance, and
`settle(public_in, public_out, fee)` (`shielded.rs:105`) moves native value
across the boundary through `ledger-math`'s proved `settle_pool`. The wire
form `ShieldedJoinSplit { anchor, nullifiers: [_; 2], commitments: [_; 2],
public_in: u64, public_out: u64, fee: u64, recipient }`
(`crates/node/src/core/payload.rs:240-254`) names no asset. A note is
`Note { value: u64, address, rho, rand }` (`crates/zk-stark/src/pool/note.rs:81-90`)
and its commitment is `compress(value_layer, nonce_digest)` (`note.rs:148`);
the nullifier is `compress(sk, nullifier_right(rho))` (`note.rs:153`). The
2-in-2-out AIR range-checks values as two 29-bit limbs of `BabyBear`, so a
note holds at most `2^58 - 1` base units (ADR-067, not implemented).

Sixteen joinsplits fit a block (`MAX_SHIELDED_PER_BLOCK`, `shielded.rs:51`,
governed as `ShieldedMaxPerBlock`). The conservation guard compares the pool
balance before and after as a movement of `NATIVE_ASSET`
(`crates/node/src/state/invariant_guard/conservation.rs:208`), and the drain
anomaly applies above `SHIELDED_DRAIN_FLOOR = 1_000_000`
(`invariant_guard/limits.rs:77`, `anomaly.rs:181-190`). Mainnet v1 ships
with `shielded_activation_height = u64::MAX` (ADR-037); feature bit 8
`SHIELDED` is claimed by `postgen/shielded-feature-bit`. `features.toml`
records `CIRCUIT_IS_AUDITED = false` for the AIR.

User-issued tokens exist as `d:asset:` records with `U256` supply and
`(asset, holder)` balances (`crates/node/src/state/asset.rs`), extended by
ADR-103 with a mint authority and cap. Zcash's ZSA drafts (ZIP 226/227) add
an asset base to the note and keep one commitment tree; Solana's Token-2022
confidential transfers encrypt balances under ElGamal (prior-art file). Both
are classical.

**Finding that contradicts the brief.** P-20 is listed as "after P-1 and the
C1 decision". P-1 is not "none" (ADR-103 found `RegisterAsset`), and the
binding limit is ADR-067: a token note cannot hold more than `2^58 - 1`
units until the 9-limb value lands, whatever its `U256` supply is in the
clear. This ADR does not wait for ADR-067; it inherits the cap and says so.

## Decision

**One pool, one tree, one nullifier set. A note carries an asset id, every
joinsplit is single-asset with the asset id as a public input, and the pool
keeps one clear balance per asset.** The native coin is asset `[0; 32]`
(`NATIVE_ASSET`, `asset.rs:14-15`), so today's notes are asset-0 notes and
nothing migrates.

### Note and circuit

- `Note { value, asset: [u8; 32], address, rho, rand }`. The asset enters the
  commitment as part of `value_layer`: `value_layer = (lo, hi, asset_digest…)`
  where `asset_digest = Poseidon2(asset)` folded into the same compression
  the value already goes through. A v1 note is a v2 note with
  `asset = [0; 32]`, and its commitment must be byte-identical: the AIR
  computes `value_layer` as today when `asset_digest` is the all-zero digest,
  which is what Zcash ZSA does for the ZEC base (prior-art file). Pinned by
  a vector.
- `JoinSplitPublic` gains `asset`. The AIR constrains both input notes and
  both output notes to that asset, and the balance equation
  `in_1 + in_2 + public_in = out_1 + out_2 + public_out + fee` is per asset
  because every term is in that asset. Mixed-asset joinsplits are refused by
  the circuit, not by the node.
- The nullifier is unchanged: `rho` is unique per note regardless of asset,
  so one nullifier set serves every asset and a token note cannot be
  double-spent as a native note.

### Wire form

`ShieldedJoinSplit` gains `asset: [u8; 32]` after `recipient`. A v1 payload
(no asset field, decoded by length) means `asset = NATIVE_ASSET`. The kind
stays `TxKind::Shielded`; no new wire version, no new frame. `fee` for a
token joinsplit is **zero**: the transaction's native fee is paid in the clear
as today's output to `FEE_COLLECTOR` (`crates/node/src/state/fees.rs:5-11`),
because a token has no fee value (ADR-103, fees stay native). A token
joinsplit with `fee != 0` is refused.

### State (invariant 25)

- `POOL_KEY` keeps the native balance, tree and anchor window, unchanged.
- New typed key `pool:a:` ‖ `asset(32)` → `U256` balance of that asset held
  inside the pool. Added to `TYPED_PREFIXES`
  (`crates/node/src/state/commitments.rs:71`) with its own leaf in the fold;
  the undo journal records prior values; `uncovered_keys()` stays empty.
- `public_in` for asset `a` debits `(a, signer)` and `public_out` credits
  `(a, recipient)` through the existing asset balance functions
  (`asset.rs`), and `settle` runs on the per-asset balance with the same
  proved `settle_pool`. An underflow on a token pool is the same tripwire
  for a soundness break that `shielded.rs:105-134` describes for native.

### Guard (invariant 28)

Still `Module::Shielded`. The conservation check (`conservation.rs:208`)
gains one `moved(ledger, a, before, after)` per asset whose `pool:a:` record
the block touched, so a circuit that minted hidden token supply fails the
block, not the module. The drain-rate anomaly stays native-only:
`SHIELDED_DRAIN_FLOOR` is in native base units and means nothing for an
arbitrary token, and a floor with no meaning is a lever (invariant 28's
reason). A token pool has no rate breaker; the conservation check is its
protection, and this is stated in `docs/invariant-guard.md`.

### Limits

- `ShieldedMaxPerBlock` counts native and token joinsplits together; the
  bound is proof bytes and verification time, which do not depend on the
  asset.
- A note value is capped at `2^58 - 1` (ADR-067). Issuers of tokens that
  need more per note choose coarser units (ADR-103 has no decimals field by
  design, `asset.rs:27-33`).

### 256-bit arithmetic

Every pool balance is a `U256` through `settle_pool`'s checked add and
subtract; the public values stay `u64` as the circuit range-checks them.
No multiplication, no division, no 512-bit intermediate.

## Alternatives considered

- **One tree per asset.** Splits the anonymity set by asset and makes the
  pool state unbounded in trees. Rejected; Zcash ZSA also keeps one tree.
- **Hidden asset id (ZSA-style).** Zcash hides the asset behind a
  value-commitment base point, which needs homomorphic commitments the STARK
  does not use, and hiding the asset loses the per-asset clear balance that
  keeps every token's supply auditable from outside (`shielded.rs:60-66`).
  Rejected for v4; a later ADR may revisit once ADR-067 lands.
- **Encrypted balances in the clear ledger (Solana Token-2022).** Twisted
  ElGamal and Bulletproofs are classical. Rejected.
- **Wait for ADR-067.** The 9-limb value is a circuit rewrite with its own
  audit; this ADR adds one public input and one note field, and the cap is
  stated rather than hidden.

## Consequences

- Token holders get the same privacy as native holders, inside the same
  anonymity set, at the same per-block cap.
- The AIR changes: a new audit of the circuit is needed before the activation
  height (`CIRCUIT_IS_AUDITED` is already false; this does not make it true).
- One typed key, one guard check per asset, one payload field with a
  length-based default, one note field with a byte-identical v1 case.
- Wallets scan with the asset id in the note plaintext; the viewing-key work
  (`features.toml` pq-viewing-keys, RESEARCH) reveals the asset alongside the
  value.

## Invariants touched

- **25**: `pool:a:` typed key in the fold; undo covered.
- **28**: `Module::Shielded`; conservation per asset; drain breaker
  native-only, stated.
- **7**: no new no-op; refusals are circuit refusals or decode refusals.
- **31**: no new frame; `TxKind::Shielded` payload grows by a trailing field.
- **32**: a joinsplit is not a vault door, unchanged.
- **33**: proof bytes unchanged; sixteen per block.
- Standing Order 4: no floats; the asset enters the circuit as field
  elements by the same digest the value uses.

## Activation

`FeatureSet::SHIELDED_ASSETS = 1 << 18` (claimed in `adr-claims.txt`),
compiled `SHIELDED_ASSETS_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**, and never before bit
8 `SHIELDED` is active. Before activation a joinsplit with
`asset != NATIVE_ASSET` is refused on admission and on apply, and the AIR in
force is the v1 AIR.

## Test plan

Red tests, `crates/node/tests/shielded_asset_tests.rs`:

- `a_native_note_commitment_is_byte_identical_under_the_v2_note`
- `a_token_joinsplit_moves_the_asset_pool_balance_and_not_the_native_one`
- `a_mixed_asset_joinsplit_is_refused_by_the_verifier`
- `a_token_joinsplit_with_a_nonzero_fee_field_is_refused`
- `a_token_note_at_two_to_the_58_is_refused_by_the_range_check`
- `a_hidden_mint_of_a_token_fails_the_block_as_a_conservation_violation`
- `a_reorg_restores_every_asset_pool_balance_and_the_nullifier_set`
- `a_token_joinsplit_before_activation_is_refused`

`crates/zk-stark`: the mutation sweep retired invariant 23 describes is
re-run over the new constraints, each with a consistent lie
(`docs/zkml.md` method). Vectors (`spec/03-state.md` SA-1..SA-4): v1 and v2
note commitments for the same native note; a token note; a single-asset
joinsplit's public inputs; a negative vector with the asset swapped on one
output. `crates/node/tests/shielded_tests.rs` keeps passing unchanged.

## Open questions for Eric (economics and pricing only)

1. Does a token joinsplit pay only the per-byte base fee, or a surcharge
   (the proof is ~0.4 MB of block space either way)?
2. Should an asset's authority (ADR-103) be able to charge a fee in the
   token on shielding, or is the native fee the only one, as written?
