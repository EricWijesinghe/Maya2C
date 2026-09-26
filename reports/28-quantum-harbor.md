# 28 — The quantum-safe harbor

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 28

> DONE WHEN: the exposure tool works on real Bitcoin and Ethereum data; PQ
> vaults work end to end on devnets with honest risk labels; the Q-day
> playbook is rehearsed in sim/; reports/28-quantum-harbor.md is complete.

| Condition | Result |
|---|---|
| Exposure tool on real Bitcoin data | **yes**: Bitcoin's genesis coinbase, whose id equals the real genesis header's merkle root |
| …on real Ethereum data | **rule implemented and tested on synthetic accounts.** No real Ethereum account data was reachable: every public RPC returned 403 (`reports/25-interop.md`) |
| PQ vaults end to end on devnets | **no devnets and no bridge.** The vault logic exists in `crates/smart-account` (delay + guardian cancel) |
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
