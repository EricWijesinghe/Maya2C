# 2026-11-10 — notary transactions admitted, then silently dropped

Branch `test/did-silent-drop`, base `f51b892a`. Tests only; the node binary is
frozen, so no node code changed.

## Report being reproduced

On a 4-validator `cargo xtask bft-devnet`, `maya2c notary setup`
(`bins/maya2c-cli/src/notary.rs` on `feat/notary`) sent a `RegisterDid` that
`send_raw_transaction` accepted; it never reached a block, the sender nonce
stayed 0, and no log said why. A later `AnchorAttestation` with the same nonce
also never landed.

## Result

`crates/node/tests/did_silent_drop_tests.rs` builds the CLI's exact
transactions (fee output sized 2 x base fee x probe size, kind set after
`Transaction::new`), admits them through `Mempool::insert` (the RPC path) and
builds blocks as `consensus::bft::build_block` does (`select_applicable`, then
`candidate_block_sealed`), on a fee-live genesis with and without the devnet's
staking section.

```
running 5 tests
test the_cli_register_did_lands_on_a_fee_live_chain ... ok
test the_cli_register_did_lands_with_the_devnets_staking_genesis ... ok
test a_stamp_before_setup_is_admitted_then_silently_dropped_for_the_missing_did ... ok
test a_second_registration_is_admitted_then_silently_dropped ... ok
test the_cli_stamp_is_admitted_then_silently_dropped_for_its_schema ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.07s
```

1. **The CLI's `RegisterDid` is valid and lands** at the state level, with and
   without the devnet's staking genesis. The reported setup drop is **not
   reproduced** here. NOT VERIFIED: the live 4-node devnet run (gossip,
   proposer split, epoch switch); not run in this session.
2. **The CLI's stamp is a CLI bug.** `notary::SCHEMA = "notary/v1"`;
   `maya_identity::CryptographicAttestation::new`
   (`crates/identity/src/attestation.rs:128-139`) allows only `[a-z0-9-]`.
   The mempool admits it; execution refuses it with
   `identity: ... "notary/v1" is not lowercase ASCII with hyphens`. Same
   transaction with `notary-v1` lands (control in the test).
3. **Node behaviour, all identity rules checked only at execution:** the
   builder discards the error. `crates/node/src/state/db.rs:1149`
   (`select_applicable_within`, `Err(_) => false`). Admission
   (`crates/node/src/network/mempool.rs:241-370`, `validate`) checks fee,
   balance, nonce, vault, signature, breaker, but never the payload's
   execution rules. Pinned cases: bad schema, stamp before any DID
   (`identity_exec.rs:210-213`), second registration (`identity_exec.rs:63-67`).
   The dropped transaction stays pooled and passes `revalidate` (the test
   asserts both), so it is re-proposed every round and never reported.

The report's "the mempool emptied" does not follow from the code: the only
removals are `step.included`, under-floor registrations and capacity eviction
(`bins/maya2c-node/src/bft.rs:925,997`). That part is unexplained and needs the
live run.

## Post-genesis issue text

> **[post-genesis] Admitted transactions dropped at block build with no reason surfaced**
>
> `Mempool::validate` checks fee, nonce, balance, vault policy, signature and
> breaker, but not a typed payload's execution rules. A transaction that fails
> those (identity schema charset, missing or duplicate DID, and every other
> payload check done only in `apply_kind_for`) is admitted, gossiped, ordered,
> then dropped by `StateDB::select_applicable_within` (`state/db.rs:1149`,
> `Err(_) => false`) with no log. It stays pooled, passes `revalidate`, and is
> re-proposed every round: the submitter sees "accepted" forever.
>
> Proposed, in order of cost:
> 1. Mempool: run the payload's stateless shape checks at admission (for
>    identity: `CryptographicAttestation::new`, `VerificationMethod::new`,
>    `ServiceEndpoint::new`) and the cheap state ones (DID exists / does not
>    exist) against committed state. Advisory, like the existing breaker check.
> 2. Builder: keep the first error per dropped txid in a bounded local map
>    and log it once (`bft: dropped <txid>: <reason>`). Not consensus: the
>    accept set does not change.
> 3. RPC: a `get_transaction_status` returning `pending` / `included@height` /
>    `dropped: <reason>` from that map.
> 4. Evict a transaction dropped N consecutive times for a non-nonce reason.
>
> Repro: `crates/node/tests/did_silent_drop_tests.rs`.

## CLI change required (feat/notary)

`bins/maya2c-cli/src/notary.rs:36`: `SCHEMA` must match `[a-z0-9-]{1,64}`,
for example `"notary-v1"`. `anchored_hash` matches on the same constant, so
both sides change together. `setup`'s payload needs no change.
