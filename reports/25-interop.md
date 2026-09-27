# 25 — Interoperability without trusted bridges

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 25

> DONE WHEN: at least the Ethereum ZK light client verifies real mainnet
> headers in tests; the trust-assumptions table is published; rate limits and
> caps pass tests; intent settlement works end to end on devnets; attack sims
> pass; reports/25-interop.md is complete.

| Condition | Result |
|---|---|
| Ethereum ZK light client verifies real mainnet headers | **partly: real finality, not ZK.** `interop::beacon` verifies a real mainnet sync-committee finality update (505/512 BLS signers) down to a Keccak-checked execution header; 64 real headers are also chained. The verification runs natively, not inside a ZK proof — see "Beacon finality" |
| Trust-assumptions table published | **yes**, `spec/interop/README.md` |
| Rate limits and caps | **pass** |
| Intent settlement end to end on devnets | **yes, by hash-locked swap** (2026-09-27): two real devnet nodes with different chain ids, the user's and the solver's watchers over JSON-RPC, settled in 8.3 s. The light-client-proof escrow variant still uses a stand-in verifier |
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

## Beacon finality (re-run 2026-09-27)

```
$ cargo test -p maya-interop --test eth_beacon_finality_tests -- --nocapture
mainnet: 505/512 sync-committee signers finalized beacon slot 15304160 → execution block 26065629 (0xcfd95d43925f2e1b9b507149f001307bdd1090e5c3f9c8677ee7be26861df226), whose Keccak header hash verifies
test a_real_mainnet_finality_update_verifies_down_to_the_execution_header ... ok
test a_forged_proof_fails_at_the_step_it_forges ... ok
```

The update, the committee and the execution header are real mainnet data
fetched into `crates/interop/tests/fixtures` (`scripts/eth_beacon_fixture.py`).
This was recorded in the master-prompt ledger when it landed and is only now
written up here. What remains for the brief's "ZK" light client: the same
checks inside a proof, so a chain can verify them cheaply. The BLS aggregate
check is classical cryptography either way.

## Intent settlement on two devnets (2026-09-27)

`bins/htlc-watcher/tests/devnet_swap_tests.rs` starts two single-validator
DAG-BFT nodes (`maya2c-devnet-a`, `maya2c-devnet-b`), **both charging fees**,
through `maya2c_cli::dev::launch`. The intent is "4,000 on A for at least
3,000 on B". Neither party starts with an account on the other chain:

1. The user's watcher locks 4,000 on A under a SHA-256 digest.
2. The solver's watcher sees it over RPC and locks 3,000 on B for the user,
   with a shorter expiry.
3. The user's claim on B reveals the preimage.
4. The solver's watcher reads it from B's state and claims on A.

No bridge, no relayer, no trusted party. Every step is a signed,
fee-paying transaction through a real node's mempool and DAG-BFT block
production.

```
$ cargo test -p maya-htlc-watcher --test devnet_swap_tests -- --nocapture
intent settled across two fee-charging devnets in 9.396056s; fees and allowances paid: user 79666 on A, solver 79666 on B
test an_intent_settles_across_two_devnets_by_hash_locked_swap ... ok
```

**What running it on real nodes found (the in-process tests bypass
admission and fees):**

1. **The watcher paid no fees.** It predates ADR-029, so on any fee-charging
   chain every lock, claim and refund it sent was refused as `FeeTooLow`. It
   now adds the fee output `l1-wallet` adds: twice the base fee on the signed
   size, read from `get_fee_info` through `SwapChain::fees`.
2. **A counterparty new to a chain could never claim.** The node refuses any
   transaction from an account that holds nothing and has never sent
   (`EmptySender`, ADR-013). A claim's fee is an output debited *before* the
   claim releases the escrow, so it cannot pay from what it claims. A first
   fix exempted such claims in the mempool. The security review found it
   reopened the free signature-verification amplification ADR-013 closed (a
   self-funded lock to a throwaway address, never settled, exempts that
   address forever), and it did not work with fees on anyway. **It was
   reverted.** Instead, each watcher pays the counterparty a **fee
   allowance** with its lock when the counterparty's account there is empty.
   The allowance is a probe claim's fee, or 1 unit on a fee-free chain. A
   watcher also waits a tick instead of claiming from its own still-empty
   account. The node's admission rule is unchanged.

The cost is visible: about 80,000 units per side at a base fee of 1 per
byte, because hybrid ML-DSA + SLH-DSA transactions are large and the fee
carries a 2× margin.
