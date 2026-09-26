# Q-day playbook

What Maya2C does when a cryptographically relevant quantum computer (CRQC)
is announced, or when a break of one of its own post-quantum schemes is
published. Master Prompt 28 §4. The lifecycle it compresses is in
`docs/CRYPTO_WATCH.md` §3.

## Which event is which

| Event | What breaks | Maya2C exposure |
|---|---|---|
| A CRQC breaks elliptic curves (Shor) | ECDSA, EdDSA, X25519 | **none for transactions**: every account signs with ML-DSA-65 and SLH-DSA. The p2p Noise X25519 layer loses confidentiality against recorded traffic, and ML-KEM inside it keeps the session keys safe (`network/pq/handshake.rs`). The dark Ed25519 suite must never be activated |
| A break of ML-DSA (lattice) | the lattice half | the hybrid still holds while SLH-DSA stands: an attacker must break both |
| A break of SLH-DSA / SHA-2 assumptions | the hash half | the hybrid still holds while ML-DSA stands |
| Both | everything | switch to the next registered suite; this is the case the forced-migration window exists for |

## Hours

1. The security council confirms the report from two independent sources.
2. Publish a status notice. For an elliptic-curve break: "Maya2C
   transactions are not affected". For a break of one PQ half: "the hybrid
   holds; migration starts".
3. For one broken half: freeze activation of any suite that uses only that
   half (governance, `DefaultSignatureSuite`).

## Days

1. MIP for the forced migration window (`spec/mips/`): the minimum
   durations of `CRYPTO_WATCH.md` §3 steps 1–4 collapse into the window.
2. Wallets and SDKs switch their default suite. Exchanges get the deposit
   notice (`docs/integrations/EXCHANGES.md`).
3. Validators upgrade to a binary that enforces the window from an
   activation height. A node that has not upgraded halts cleanly: "upgrade
   required before height H".

## Weeks

1. The migration runs: accounts rotate to the new suite, and the address
   stays the same where the account type allows (smart-account rotation).
2. At the end of the window, the broken suite's signatures are rejected
   from an activation height.

## Rehearsed

The suite-migration mechanism, in sim:

```
$ cargo test -p maya-crypto-pq --profile ci --test agility_migration_tests -- --nocapture
migration SIM: 1000000 accounts; window actions in 500 blocks; sweep in 41 blocks (≤25000 examined/block); 200000 vault claims, 100000 locked; 200 transfers every block, none failed
test a_million_accounts_migrate_without_downtime ... ok
```

The halt-before-fork upgrade is rehearsed in
`crates/node/tests/upgrade_rehearsal.rs`. The communication steps and the
council's hours-scale response have **not** been rehearsed with people.
