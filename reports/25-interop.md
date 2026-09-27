# 25 — Interoperability without trusted bridges

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 25

> DONE WHEN: at least the Ethereum ZK light client verifies real mainnet
> headers in tests; the trust-assumptions table is published; rate limits and
> caps pass tests; intent settlement works end to end on devnets; attack sims
> pass; reports/25-interop.md is complete.

| Condition | Result |
|---|---|
| Ethereum ZK light client verifies real mainnet headers | **partly.** 64 consecutive real Prague-era mainnet headers (26065574–637) are recomputed from their fields and chained, plus the genesis header. There is **no ZK and no finality (sync-committee) verification** |
| Trust-assumptions table published | **yes**, `spec/interop/README.md` |
| Rate limits and caps | **pass** |
| Intent settlement end to end on devnets | **no devnets.** The escrow logic passes with a stand-in verifier |
| Attack simulations | **pass** for the ones modelled (below) |

## Ethereum headers

```
$ cargo test -p maya-interop --profile ci -- --nocapture
keccak256(rlp(mainnet genesis header)) = d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3
test the_real_mainnet_genesis_header_hashes_to_its_published_hash ... ok
test a_forged_field_or_a_broken_parent_link_is_caught ... ok
test a_route_bug_cannot_move_more_than_its_cap_and_trust_grows_only_with_clean_time ... ok
test a_solver_that_takes_the_intent_and_never_delivers_gets_nothing_and_the_user_is_refunded ... ok
test delivery_settles_whoever_submits_the_proof_so_relayer_censorship_does_not_block_it ... ok
test result: ok. 5 passed; 0 failed …
```

The genesis fields come from go-ethereum's `DefaultGenesisBlock`; the
expected hash is geth's `MainnetGenesisHash`, fetched from GitHub. The test
is self-verifying: a wrong field cannot hash to that value.

**Why only one real header.** Every public Ethereum RPC and explorer API tried
returned HTTP 403 from this environment's egress proxy (publicnode,
Cloudflare, LlamaRPC, Ankr, Blockscout). GitHub was reachable, and geth
pins the genesis there. So the post-London trailing fields (base fee,
withdrawals, blobs, beacon root, requests hash) are implemented but **not
checked against a real header**, and neither is a multi-block mainnet chain.
Parent linkage is tested on synthetic children.

**What a "light client" would add.** Verifying that a header is *final*
means checking beacon-chain sync-committee BLS signatures. That is classical
cryptography; wrapping it in a STARK, as the brief asks, is not built. So
today's claim is exactly: "a header whose hash and parent link check".

## Attack simulations

| Attack | Result |
|---|---|
| Forged Ethereum header (one flipped bit) | hash no longer matches: rejected |
| Header not linked to its parent | `check_chain` rejects it at that index |
| Bitcoin reorg deeper than confirmation depth | `crates/btc-spv/tests/spv_tests.rs` has the 6-block reorg test (from earlier this branch) |
| Solver takes the intent and never delivers | claim refused without a proof; user refunded after the deadline; no late claim |
| Relayer censorship | anyone can submit the proof; settlement is not tied to a relayer |
| A bug in a route | loses at most the route's cap (10,000 on day zero in the test); an incident resets the cap |

The intent tests use a **stand-in verifier** (it accepts a proof equal to the
intent's commitment). With a real destination-chain light client, a "proof of
delivery" is only as strong as that light client, which for Ethereum is not
built.

## Not done

- ZK light clients (Ethereum, Solana, Cosmos) and the IBC v2 evaluation ADR.
- A one-balance wallet view across chains.
- Measured Maya2C ↔ Ethereum transfer time and cost.

## Real mainnet headers (added 2026-09-27)

`scripts/eth_headers_fixture.py` fetched 64 consecutive headers from a public
endpoint into `crates/interop/tests/fixtures/eth_mainnet_headers.json`.

```
$ cargo test -p maya-interop --test eth_mainnet_headers_tests -- --nocapture
mainnet blocks 26065574..=26065637 (64 headers, fetched 2026-09-27T01:54:20+00:00 from https://ethereum-rpc.publicnode.com): every hash recomputed, every parent link holds
test sixty_four_real_mainnet_headers_hash_to_their_ids_and_link ... ok
test a_real_header_with_one_field_changed_no_longer_links ... ok
```

These exercise every optional field through Prague (base fee, withdrawals,
blob gas, parent beacon root, requests hash). A valid-looking chain is not a
finalized one: finality is the beacon chain's sync-committee signature, which
this crate does not verify, and nothing here is a ZK proof.
