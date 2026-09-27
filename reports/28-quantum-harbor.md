# 28 — The quantum-safe harbor

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 28

> DONE WHEN: the exposure tool works on real Bitcoin and Ethereum data; PQ
> vaults work end to end on devnets with honest risk labels; the Q-day
> playbook is rehearsed in sim/; reports/28-quantum-harbor.md is complete.

| Condition | Result |
|---|---|
| Exposure tool on real Bitcoin data | **yes**: Bitcoin's genesis coinbase, whose id equals the real genesis header's merkle root |
| …on real Ethereum data | **yes** (2026-09-27): 200 accounts from mainnet blocks 26065698–99, and a sender's public key recovered from a real signature — see "Real Ethereum data" below |
| PQ vaults end to end on devnets | **yes, for the native coin** (2026-09-27): vault accounts in the node (ADR-030) on a real devnet, with the honest risk label in `maya2c vault`. **No outside-asset bridge route exists**, so bridging BTC/ETH into a vault is not built |
| Q-day playbook rehearsed in sim | **the mechanism is**: 1,000,000-account suite migration, no failed transfers. The human steps are not |

## Exposure tool

```
$ cargo test -p maya-quantum-harbor --profile ci -- --nocapture
genesis coinbase 4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b: 50 BTC in P2PK — Your public key is already on the blockchain. A large enough quantum computer could compute your private key from it. …
test bitcoins_genesis_output_is_a_p2pk_with_its_public_key_exposed ... ok
test each_template_is_classified_by_exact_shape ... ok
test an_ethereum_account_is_exposed_once_it_has_signed ... ok
test malformed_transactions_are_refused_not_guessed ... ok
test result: ok. 4 passed; 0 failed …
```

- **Bitcoin.** The tool classifies each output by template. P2PK and P2TR
  are exposed: a Taproot output key *is* a public key. P2PKH and P2WPKH are
  hashed until spent. P2SH and P2WSH depend on the hidden script.
  Non-templates are "unknown", never guessed.
- **Ethereum.** An EOA is exposed once it has signed anything (nonce > 0);
  contracts have no key.
- **Every classification comes with plain-language advice.**

**Real data.** The raw genesis coinbase is embedded in the test. Its
double-SHA256 is checked against `4a5e1e4b…a33b`, the merkle root in the
real genesis header that `crates/btc-spv` verifies, so the test cannot pass
on a mistyped transaction.

**Aggregate dashboards are not built.** An aggregate of exposed value
across a chain needs a full UTXO set or an archive node, and neither is
reachable here. The public estimate of ~6.26M BTC with exposed keys
(`docs/strategy/WEAKNESS_MAP.md` row 5) is Chaincode Labs' number, not ours.

## The honest limit, as it must appear in any vault UI

A bridged asset is only as safe as the weakest of: the source chain, the
bridge route, and Maya2C. If a quantum attacker breaks the source chain
itself, the bridged representation is affected too. A Maya2C vault protects
against theft of **your own exposed key**, and nothing more. No bridge
exists yet (`reports/25-interop.md`), so no vault holds an outside asset.

## Q-day playbook

`docs/security/Q_DAY_PLAYBOOK.md` covers hours, days and weeks, for four
cases:

- an elliptic-curve break;
- a lattice break;
- a hash break;
- both.

The case that matters most is stated plainly: **an elliptic-curve break
does not affect Maya2C transactions**, because none sign with an elliptic
curve.

```
migration SIM: 1000000 accounts; window actions in 500 blocks; sweep in 41 blocks (≤25000 examined/block); 200000 vault claims, 100000 locked; 200 transfers every block, none failed
test a_million_accounts_migrate_without_downtime ... ok
```

## Not done

- Long-horizon custody configurations (archival SLH-DSA seals, scheduled
  re-sealing).
- Custodian reporting.
- Publishing the crypto-agility crates for others' use. Licensing and
  publishing need a decision and "APPROVED: publish".

## Real Ethereum data (added 2026-09-27)

Fetched from `https://ethereum-rpc.publicnode.com` by
`scripts/eth_exposure_fixture.py`, pinned at block 26,065,699, committed as
`crates/quantum-harbor/tests/fixtures/eth_mainnet_sample.json`. Windows 11,
Intel family 6 model 198.

```
$ cargo test -p maya-quantum-harbor --test ethereum_mainnet_tests -- --nocapture
block 26065621: 0x0d95b7d8…f2e0 signed by 0xc917c3fa…5249 — public key 0458cba909179de21a9d898c… recovered from the signature
mainnet block 26065699 sample (2 blocks): 200 accounts — 135 exposed EOAs, 9 never-signed EOAs, 56 contracts; 7 of the exposed are EIP-7702-delegated EOAs that a code check alone would call keyless; 177196.96 of 2235876.60 ETH (7.9%) sits behind exposed keys
test result: ok. 2 passed; 0 failed
```

(The signed-transaction line names the transaction captured by the first
fetch; the account sample was refetched with code prefixes. Both are real.)

**What the real data changed.** The first version classified any account
with code as "no key". Seven of the 200 accounts are EIP-7702-delegated EOAs
— code `0xef0100…` — which have a private key and signed the delegation, so
their key is exposed. `ethereum_exposure_of` now reads the designator.

**What the sample is not.** Two blocks of active accounts are biased toward
accounts that transact, and most ETH in the sample sits in contracts. It is a
demonstration that the tool works on live data, not a chain-wide estimate.

## PQ vaults on a devnet (2026-09-27)

A vault is an account's opt-in policy (ADR-030): a delay in blocks, an
instant limit per window, and guardians. Its keys are the chain's hybrid
ML-DSA-65 + SLH-DSA keys, so both the owner's signature and the guardian's
cancel are post-quantum. Over-limit withdrawals are escrowed and wait out the
delay, and any guardian can cancel them. Nothing but a plain transfer or a
vault action can leave a vault.

```
$ cargo test -p custom-l1-node --test vault_tests
test the_fee_collector_is_not_a_way_around_the_limit ... ok
test bad_policies_and_requests_are_refused ... ok
test the_limit_bounds_a_window_not_a_transaction ... ok
test a_matured_request_pays_and_only_once ... ok
test a_stolen_key_waits_and_a_guardian_cancels ... ok
test reconfiguration_waits_out_the_current_delay_and_can_be_cancelled ... ok
test result: ok. 6 passed; 0 failed

$ cargo test -p maya2c-cli --test vault_devnet_tests
test result: ok. 1 passed; 0 failed   (a real node; every step through its mempool)
```

The devnet test is the stolen-key story end to end:

1. The over-limit transfer is refused at admission, with the reason.
2. The thief's 900,000,000 request is escrowed and cancelled by the guardian.
3. The owner's 5,000 request waits five blocks and is then executed by a
   third party.

`maya2c vault <address>` prints the policy, the open requests and this risk
label:

> A vault protects against theft of your own key: withdrawals above the
> limit wait out the delay, and any guardian can cancel them. It does NOT
> protect against a thief who also holds a guardian key, or against transfers
> within the per-window limit, which are instant. An asset bridged into a vault
> is only as safe as the weakest of its source chain, the bridge route and
> Maya2C; if the source chain is broken, the bridged representation is
> affected too.

**Review changed the design before it landed.** The rust and security
reviewers found that fee outputs, excluded from the limit and uncapped, let
a stolen key send the whole balance to the fee collector in one instant
transaction. They also found that a per-transaction limit let thousands of
in-limit transfers drain a vault in one round. Both are fixed (a fee
headroom of 4x the required fee, and a per-window limit) and pinned by
tests. Settled requests are deleted, so state stays bounded. The circuit
breaker can halt vault configuration, requests and execution, but never a
cancel. Details and remaining limits: ADR-030.

**Not built:** bridging outside assets into a vault (no Master Prompt 25
route exists for BTC or ETH), SLH-DSA archival re-sealing schedules, and
custodian compliance reports.

