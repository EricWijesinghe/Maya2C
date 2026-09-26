# Integrating Maya2C at an exchange

Status: **no public network exists.** Everything below describes the node as
it runs today on a private or local network, and says where mainnet will
differ. Master Prompt 17 §1.

## Finality, in plain numbers

Today the node runs Nakamoto proof of work with a 15-second target spacing.
Finality is **probabilistic**: a deposit is irreversible with a probability
that grows with confirmations. DAG-BFT finality (ADR-015) is not wired in.

Probability that an attacker with hash-rate share *q* reverses a deposit
after *z* confirmations (the formula from the Bitcoin whitepaper, §11):

| Attacker share q | P < 10⁻³ | P < 10⁻⁶ |
|---|---|---|
| 10 % | 5 conf. (1.2 min) | 11 conf. (2.8 min) |
| 25 % | 15 conf. (3.8 min) | 31 conf. (7.8 min) |
| 30 % | 24 conf. (6.0 min) | 49 conf. (12.2 min) |
| 40 % | 89 conf. (22.2 min) | 184 conf. (46.0 min) |

Choose *q* from how much hash rate you believe could be rented against the
chain, which on a young PoW network is most of it. Treat this table as a
floor, not a promise. Under DAG-BFT, finality becomes deterministic after the
commit (target p50 ≤ 2 s, `docs/SLO.md`) and this section will be rewritten.

## Deposit addresses and memos

- **One address per customer.** An address is `blake3` of a hybrid
  ML-DSA-65 + SLH-DSA public key (`spec/02-transactions.md` TX-2), so it
  costs nothing to create and needs no on-chain registration.
- There is **no memo or tag field** in the v5 transfer (`spec/01-encoding.md`).
  Shared-address-plus-memo schemes are not possible and are not needed.

## Crediting deposits

`crates/deposit-watcher` is the reference service. It reads blocks at or
below your chosen finalized height and credits each deposit exactly once
across crashes. It was tested with 100,090 deposits and 40 SIGKILLs:
the ledger matched ground truth exactly (`reports/17-integrations.md`).

## Withdrawals and fees

- A transfer carries any number of outputs (`spec/03-state.md`), so batching
  withdrawals into one transaction is native: one signature (11,165 B) for
  many 40-byte outputs.
- The fee market is **inactive** (`FeeConfig::DISABLED`). Transactions pay no
  fee today. When it activates, the fee is `base_fee × size_bytes` (FEE-5),
  so a batched withdrawal amortises the signature's size across its outputs.
- Nonces are strict (STF-1): one account, one sequence. Run withdrawals from
  one hot account serially, or use several hot accounts in parallel.

## Node requirements

`docs/NODE_TYPES.md` (full node row). Run your own node. The deposit watcher
must read a node you control, never a public RPC.

## A chain halt

If blocks stop, deposits stop confirming; nothing is lost. A node that meets
a protocol upgrade it does not support halts with "upgrade required before
height H" (`reports/15-spec.md`): upgrade the binary, restart, and it catches
up. Pause withdrawals while your node is not at the tip.
