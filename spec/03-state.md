# 3. State transition — v0.1.0

The accounts-only transfer path. Reference: `crates/spec-ref/src/{stf,root}.rs`.
Node: `StateDB::stage_transaction`, `StateDB::apply_block`,
`state::merkle`.

An account is `(balance: u64, nonce: u64)`. A transfer is applied in this
order, and the first rule that fails is the result:

- **STF-1** The transaction's nonce equals the sender's current nonce; otherwise `InvalidNonce`. (The replay and double-spend guard.)
- **STF-2** The sum of the outputs fits in a `u64`; otherwise `BalanceOverflow`.
- **STF-3** The sender's balance is at least that sum; otherwise `InsufficientBalance`.
- **STF-4** The sender is debited and its nonce advanced by one; a nonce at `u64::MAX` cannot advance and is `BalanceOverflow`.
- **STF-5** Each recipient, in output order, is credited; a credit past `u64::MAX` is `BalanceOverflow`.
- **STF-6** The debit is written before any credit is read, so a self-transfer nets to zero and cannot mint. *(positive only: no input is rejected by this rule)*
- **STF-7** An address with no record reads as balance 0, nonce 0; a credit creates the record.
- **STF-8** A block applies all its transactions in order, each seeing the effects of those before it, or it applies none: one failing transaction makes the block invalid and leaves state untouched.

## State root

- **ROOT-1** An account's leaf is `blake3(0x00 ‖ address ‖ balance_u64 ‖ nonce_u64)`. *(positive only: defines a value)*
- **ROOT-2** An internal node is `blake3(0x01 ‖ left ‖ right)`. *(positive only: defines a value)*
- **ROOT-3** Leaves are paired left to right; an unpaired last node is promoted unchanged, not duplicated; this repeats until one node remains. With no leaves the root is 32 zero bytes. *(positive only: defines a value)*
- **ROOT-4** Leaves are ordered by ascending address. With no other state layer present, the state root is this accounts root. *(positive only: defines a value)*
- **ROOT-5** Each further layer (channels, notes, trading, governance, …) is folded in, in the order `state::proof::StateLayer` fixes, only once it holds a record, so a chain that never used a layer keeps the root it always had. *(positive only: defines a value)*

## Gaps

- ROOT-5 is pinned by the accounts-only vectors: the node folds every layer it has and must still reproduce the reference's accounts-only root. No vector yet populates a layer.
- The end-of-block passes (sealed swaps, trading, oracle, governance,
  invariant guard) run after the transfers; with no records they are inert,
  which is what the vectors exercise. Their own rules are specified by code.
- The sparse accounts tree (stateless mode, dark) has no vectors.
