# ADR-133: A paid storage market with possession proofs — options on top of blobs and the archive

**Status:** OPTIONS FOR ERIC, 2026-11-12. Nothing is decided here. Every option but A depends on ADR-123 (blobs) and is a post-genesis dark feature.
**Date:** 2026-11-12
**Related:** parity row P-36, [prior-art/storage-market.md](../prior-art/storage-market.md), ADR-123 (blob transactions with expiry, branch `docs/adr-117-124-gaps`), ADR-127 (VRF host function, branch `docs/adr-125-130-gaps`), ADR-129 (compute jobs with escrow, same branch), ADR-132 (duties, this branch), [ADR-044](ADR-044-mining-rewards-lane.md); invariants 11, 17, 24, 25, 26, 27, 28

## Context

What stores bytes today, and for whom:

- **`crates/archive` stores Maya2C's own history, verified but unpaid.**
  A batch of pruned blocks becomes one CAR v1 file whose root is a manifest
  linking every block by CID; it can live in a local directory, an IPFS
  (kubo) node or behind an Arweave gateway, and `open_archive` checks every
  section against its CID so a store "can refuse to answer but cannot
  substitute bytes" (`crates/archive/src/lib.rs:1-17`; `arweave.rs:24`,
  `kubo.rs`, `store.rs:77`). REAL, verified (`features.toml:2024-2031`).
  Nobody is paid and nothing proves a store still holds the file; the
  node reads it back before deleting its own copy (invariant 27) and
  that is the end of the check.
- **`crates/da` can prove a cell is present against a root.** `k × k`
  square, Reed–Solomon to `2k × 2k`, BLAKE3 row and column roots, one
  `data_root` (`crates/da/src/lib.rs:1-27`, `square.rs:19`), `cell(row, col) → (bytes, CellProof)`
  (`square.rs:112`), `repair` (`:137`), `sample` (`:209`). RESEARCH, working
  (`features.toml:3120-3128`); "nothing in consensus produces or checks
  these squares yet" (`lib.rs:27`). A `CellProof` is a Merkle path of
  `log2(2k)` hashes: hash-only, so it is the post-quantum possession proof
  this ADR would need, and it exists.
- **ADR-123 gives blobs a lifetime and a fee, and no market.** A blob is
  priced by bytes, its `data_root` is a record under the state root for
  `BLOB_RETENTION_BLOCKS`, the bytes are local-only (`blob:` prefix) and
  deleted at expiry without archive. Validators hold up to
  `2 MiB × BLOB_RETENTION_BLOCKS`. After expiry nobody is obliged to hold
  anything. ADR-123's open question 2 asks Eric for the retention window.
- **Escrow-settled work exists on paper.** ADR-129 (compute jobs) writes
  `cj:` records with escrow, bonds and byte-equality settlement at block
  end; ADR-044 forbids paying anything from issuance. A storage market is
  the same shape with a different proof.
- **The oracle is absent by default** (invariant 11). "The provider served
  the file" cannot be a claim from outside; it must be a proof the chain
  checks.

**Findings that contradict the brief.**

1. "Possession" and "retrievability" are different claims. A `CellProof`
   shows a provider held one cell at challenge time; it does not show the
   provider will *serve* the file to a user. Filecoin's PDP (mainnet
   2026-03-26, prior-art file) is possession only and pairs it with
   off-chain retrieval SLAs; Storj audits are off-chain. No option below
   proves retrievability, and the ADR says so rather than naming the
   feature "proof of retrievability".
2. The row cites invariant 26 (blocks and state commit in one batch).
   Nothing here touches the block store; 26 is reached only in that a
   market record commits in the block's batch like any other record.
3. "Permanent storage" (Arweave's endowment) needs a forecast of future
   storage cost declines (Arweave assumes about 0.5%/yr, prior-art file).
   A forecast is a float and a wall-clock assumption; it cannot be a
   consensus rule here (Standing Order 4, invariant 9). Option D records
   why it is refused.
4. `crates/da` is RESEARCH. ADR-123's own vote precondition is what moves
   it; this ADR adds no second path out of RESEARCH.

## Options

### A. None: ADR-123 blobs with expiry, `crates/archive` for history

Blobs live `BLOB_RETENTION_BLOCKS`; anyone who wants longer keeps a copy.
Maya2C's own history is archived by operators at their own cost, as now.

### B. Paid retention extension on validators (no new actor)

A `ExtendBlob { data_root, blocks }` transaction pays
`blocks × bytes × blob_byte_fee` into the fee collector and moves the
record's `expiry` forward, bounded by `MAX_BLOB_EXTENSION_BLOCKS`
(governed). Validators already hold the bytes; the market is "pay the
committee to keep holding". Possession is implied by the DA attestation
ADR-123 already defines (a sampler can ask any validator for a cell).
Nothing new is proved; the retention is the product. Cheapest option
with a market in it.

### C. Provider market with on-chain possession challenges (Filecoin PDP shape)

Prefix `st:`, layer `StateLayer::Storage`, breaker `Module::Storage`:

- `st:p:` ‖ provider → `Provider { bond: U256, endpoint_hash, faults: u32 }`;
  bond ≥ `min_provider_bond` (governed).
- `st:d:` ‖ data_root → `Deal { client, provider, bytes, k, price_per_byte_epoch: U256,
  escrow: U256, paid_until: u64, last_proof: u64, misses: u16 }`. A deal
  is opened against a `data_root` that is or was an ADR-123 blob (so the
  encoding is known and `crates/da` can rebuild it); the client funds
  `escrow`; the provider accepts and must fetch the bytes before expiry.
- **Challenge per epoch.** At the epoch boundary the chain derives, for
  every live deal, `(row, col) = H(beacon ‖ data_root ‖ epoch) mod (2k)²`
  from the block randomness (ADR-127's beacon; invariant 9: a height, not
  a clock). The provider has `PROOF_WINDOW_BLOCKS` to land
  `ProveStorage { data_root, cell, proof: CellProof }`; the node checks the
  proof against the row root listed under `data_root` (ADR-123's record
  keeps the row roots or the chain re-derives them from the header). A
  valid proof pays `price_per_byte_epoch × bytes` from escrow to the
  provider; a miss increments `misses` and after `MAX_MISSES` the deal
  closes, the remaining escrow returns to the client and
  `provider_fault_slash_bps` of the bond is burned.
- Payment is from escrow (ADR-044: nothing minted). Settlement at block
  end like ADR-129.
- Two copies: a client opens two deals with two providers if it wants
  Filecoin Onchain Cloud's default replication; the chain does not pick
  providers.

### D. Permanent endowment (Arweave shape)

Pay once, stored forever, miners paid from an endowment whose rate assumes
storage costs fall. Refused for the reason in finding 3; listed so the
refusal is on record.

## Recommendation

**A until ADR-123 is live; then B; C only when someone outside the
project asks to be paid to store Maya2C data.** Reasons:

1. B is one transaction and one field change on a record ADR-123 already
   defines. It gives users the thing a market sells, more time, with no
   new actor, no bond, no challenge loop, and no breaker to size.
2. C is the right shape (its proof exists in `crates/da`, its escrow in
   ADR-129's pattern, its slashing in ADR-132 option B's caps if
   providers are validators, or a plain bond if not), and it is a lot of
   consensus code for a demand nobody has measured. Filecoin's own
   demand-side number after its rebuild is an annualized run rate of
   about $138K across 149 payers (Messari, October 2026, prior-art file).
   That is the market size at the incumbent; Maya2C's is MISSING.
3. C's challenge is possession, not retrieval. Selling it as "your file
   is safe" is a claim the chain cannot back; selling it as "a bonded
   party proved it still holds a random cell this epoch" is accurate and
   less attractive. Eric should know that before choosing C.
4. D is refused on determinism grounds, not taste.

## Consequences per option

| | A none | B extension | C provider market | D endowment |
|---|---|---|---|---|
| New actor | none | none | providers with bonds | miners paid from endowment |
| New state | none | none (field on ADR-123's record) | `st:p:`, `st:d:` | an endowment balance + a forecast |
| What is proved | ADR-123 attestation during retention | same, for longer | a random cell per epoch per deal | replication by incentive, unproved |
| Validator storage | ≤ 2 MiB × retention | grows with paid extensions, capped by `MAX_BLOB_EXTENSION_BLOCKS` | unchanged; providers hold deals | unbounded |
| Apply-time cost | none | one record write | one Merkle check per proof, bounded by `MAX_PROOFS_PER_BLOCK`; one hash per live deal per epoch | n/a |
| Payment source | n/a | fee collector | client escrow | endowment |
| Determinism | n/a | integer | integer; challenge from beacon | **needs a cost forecast** |

## Invariants touched

- **9**: challenges are drawn by height from the beacon, never by time.
- **11**: no oracle; a proof is checked by the node.
- **17**: `blob_byte_fee`, `MAX_BLOB_EXTENSION_BLOCKS`, `min_provider_bond`,
  `PROOF_WINDOW_BLOCKS`, `MAX_MISSES`, `provider_fault_slash_bps`,
  `MAX_PROOFS_PER_BLOCK` governed, read from state.
- **24**: the market record is under the root; the blob bytes stay
  outside it (ADR-123).
- **25**: `st:` → `RECORD_LAYERS`, `StateLayer::Storage`, undo coverage for
  escrow movements and fault counters.
- **26**: untouched; records commit in the block batch like any other.
- **27**: untouched; ADR-123 already placed blobs outside "archive before
  delete". A provider that stores *history* (CAR files) is outside this
  ADR; `crates/archive` is not a deal.
- **28**: `Module::Storage`; a tripped breaker halts proof settlement and
  deal opening, never an escrow refund path that returns a client's money
  (a halted module must not freeze funds).

## 256-bit math

`price_per_byte_epoch × bytes`: `bytes ≤ MAX_BLOB_BYTES < 2²¹` (ADR-123:
`k ≤ 64`, 256-byte chunks → ≤ 1 MiB), price ≤ 10²⁴ < 2⁸⁰, product < 2¹⁰¹.
`blocks × bytes × blob_byte_fee` (option B): blocks < 2⁶⁴, so < 2¹⁶⁵.
Both checked `U256`, no 512-bit intermediate, bound written at the call
site (memory rule 2026-11-01).

## Test plan if adopted

Option B, `crates/node/tests/blob_extension_tests.rs`:

- `extending_a_blob_moves_expiry_and_charges_blocks_times_bytes_times_fee`
- `an_extension_past_max_blob_extension_blocks_is_refused`
- `an_expired_blob_cannot_be_extended` (bytes are gone)
- `a_reorg_restores_the_previous_expiry`

Option C, `crates/node/tests/storage_market_tests.rs`:

- `before_the_height_no_st_record_exists_and_the_root_matches_master`
- `a_deal_needs_a_known_data_root_and_a_bonded_provider`
- `the_challenge_cell_is_a_function_of_beacon_data_root_and_epoch_and_nothing_else`
- `a_valid_cell_proof_pays_from_escrow_and_never_more_than_escrow`
- `a_forged_cell_proof_against_the_wrong_row_root_is_refused`
- `max_misses_closes_the_deal_refunds_the_client_and_burns_the_slash`
- `a_tripped_storage_breaker_halts_settlement_and_not_refunds` (28)
- `proofs_per_block_are_capped` (33: block bytes)
- `exploit_replays::a_provider_that_keeps_one_row_only_is_caught_within_n_epochs` — prints the measured `n`, asserts nothing against a target.
- `crates/da/tests/da_tests.rs`: a `CellProof` vector at `k = 64` shared with ADR-123.

Spec: a "storage" section, vectors ST-1..ST-4.

## Activation

`FeatureSet::STORAGE_MARKET = 1 << 39` (reserved in `adr-claims.txt`;
option B can ride ADR-123's bit 30 as a field change instead), compiled
height `u64::MAX`, scheduled under ADR-082 after `BLOBS` (bit 30) is live.

## Decision needed from Eric

1. A, B or C? (D is refused on determinism grounds unless he overrules.)
2. If B: `MAX_BLOB_EXTENSION_BLOCKS` — how long may a paid blob sit on
   every validator? The storage figure in ADR-123's consequences is the
   lever.
3. If C: must providers be validators (ADR-132 option B, slashing under
   its caps) or any bonded address?
4. If C: is a burned fault slash acceptable, or should it go to the client?
5. Is the phrase "proof of possession, not retrieval" acceptable in every
   user-facing string? (The chain cannot prove retrieval.)
