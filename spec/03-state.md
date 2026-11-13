# 3. State transition — v0.1.2

The accounts-only transfer path. Reference: `crates/spec-ref/src/{stf,root}.rs`.
Node: `StateDB::stage_transaction`, `StateDB::apply_block`,
`state::merkle`.

An account is `(balance: u64, nonce: u64)`. A transfer is applied in this
order, and the first rule that fails is the result:

- **STF-1** The transaction's nonce equals the sender's current nonce; otherwise `InvalidNonce`. (The replay and double-spend guard.)
- **STF-2** The sum of the outputs, each 256-bit (ADR-046), fits in 256 bits at every step; otherwise `BalanceOverflow`.
- **STF-3** The sender's balance is at least that sum; otherwise `InsufficientBalance`.
- **STF-4** The sender is debited and its nonce advanced by one; a nonce at `u64::MAX` cannot advance and is `BalanceOverflow`.
- **STF-5** Each recipient, in output order, is credited; a balance is 256 bits (ADR-046), and a credit past `2^256 − 1` is `BalanceOverflow`.
- **STF-6** The debit is written before any credit is read, so a self-transfer nets to zero and cannot mint. *(positive only: no input is rejected by this rule)*
- **STF-7** An address with no record reads as balance 0, nonce 0; a credit creates the record.
- **STF-8** A block applies all its transactions in order, each seeing the effects of those before it, or it applies none: one failing transaction makes the block invalid and leaves state untouched.

## State root

- **ROOT-1** An account's leaf is `blake3(0x00 ‖ address ‖ balance_u256 ‖ nonce_u64)`: the balance as 32 bytes little-endian (ADR-046), the nonce as 8. *(positive only: defines a value)*
- **ROOT-2** An internal node is `blake3(0x01 ‖ left ‖ right)`. *(positive only: defines a value)*
- **ROOT-3** Leaves are paired left to right; an unpaired last node is promoted unchanged, not duplicated; this repeats until one node remains. With no leaves the root is 32 zero bytes. *(positive only: defines a value)*
- **ROOT-4** Leaves are ordered by ascending address. With no other state layer present, the state root is this accounts root. *(positive only: defines a value)*
- **ROOT-5** Each further layer (channels, notes, trading, governance, …) is folded in, in the order `state::proof::LAYER_ORDER` lists (the order `StateDB::state_layers` folds: channels, then the record layers in `RECORD_LAYERS` order, with stateless `sl:` before upgrade-schedule `up:`, then shielded, contracts, nullifiers; not the `StateLayer` declaration order), only once it holds a record, so a chain that never used a layer keeps the root it always had. *(positive only: defines a value)*

## Security council

Present only when genesis names a council. Node: `StateDB::apply_council`,
`core::council_payload`. Constants: `crates/governance/src/limits.rs`.

- **CNL-1** A council action lands only with at least `threshold` approvals from distinct members of the council in force at the block's height, each an ML-DSA-65 signature over the council domain, the chain tag, the action and the record's current nonce; a landed action advances the nonce by one. *(external vectors: crates/node/tests/council_pause_tests.rs)*
- **CNL-2** A council pause or resume naming Governance (module tag 4) is refused. *(external vectors: crates/node/tests/council_pause_tests.rs)*
- **CNL-3** A council pause of a module is refused before the end of the last council pause of that module plus `COUNCIL_RENEWAL_COOLDOWN_BLOCKS`; a council resume ends the pause at its own height. A council sunset is refused before the council's last sunset height plus the same cooldown. *(external vectors: crates/node/tests/council_pause_tests.rs)*
- **CNL-4** A council rotation names 1 to 32 distinct valid ML-DSA-65 keys and a threshold above half of them and at most all of them, and an activation from `COUNCIL_ROTATION_LEAD_BLOCKS` to `MAX_SCHEDULE_HORIZON_BLOCKS` after the block that carries it. It replaces any pending rotation. From the activation height the new members and threshold are the council in force, and the nonce carries on. *(external vectors: crates/node/tests/council_pause_tests.rs)*

## Gaps

- ROOT-5 is pinned by the accounts-only vectors: the node folds every layer it has and must still reproduce the reference's accounts-only root. No vector yet populates a layer.
- The end-of-block passes (sealed swaps, trading, oracle, governance,
  invariant guard) run after the transfers; with no records they are inert,
  which is what the vectors exercise. Their own rules are specified by code.
- The sparse accounts tree (stateless mode, dark) has no vectors.
