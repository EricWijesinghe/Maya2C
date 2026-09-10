# The Maya2C pool service

A mining pool: a Stratum V2 listener that validates shares against the chain's
own proof-of-work rule, an append-only ledger recording what each share was
worth, a PPLNS engine turning those records into credits, and a payout engine
settling them as ordinary signed transfers.

Implemented in [`pool-service/`](../pool-service). The protocol it speaks is
described in [`stratum-v2.md`](stratum-v2.md), which this document assumes.

---

## 1. There is no block reward, and everything downstream follows from that

`grep -i 'coinbase|subsidy|reward'` across `src/` returns nothing. `Block` is
`{header, transactions}`, `apply_block_checked` stages only the block's own
transactions, and fees burn to `FEE_SINK = [0u8; 32]`
([`src/state/shielded.rs:342`](../src/state/shielded.rs)). Nothing is minted for
finding a block.

A pool splits a block reward. There is not one. So:

- **Share credits accrue in weight.** A share proving `2^b` hashes is worth
  `2^b`, and the PPLNS window is a quantity of work rather than a number of
  shares.
- **Coin enters only at settlement,** from a treasury account the operator
  funds, through ordinary signed transfers the chain already supports with no
  consensus change.
- **`--reward-per-block` has no default.** What a found block distributes is
  operator policy. A plausible-looking default would be a policy invented by the
  binary; the daemon refuses to start without one
  ([`config.rs`](../pool-service/src/config.rs)).

A coinbase or block subsidy would make mining self-sustaining and remove the
treasury entirely. It is a hard fork — it changes `apply_block_checked` and the
supply schedule — and is deliberately out of scope rather than smuggled in.

## 2. What the operator has to decide

| Flag | Meaning | Failure if wrong |
|---|---|---|
| `--reward-per-block` | Value one found block distributes | Refuses to start when unset |
| `--fee-rate` | Operator's cut, `[0, 1)` | Refuses to start outside the range; `1.5` would otherwise pay miners nothing |
| `--pplns-factor` | Window as a multiple of network difficulty | Refuses non-positive or non-finite |
| `--confirmations` | Depth before credits are payable | Refuses zero: paying on a block that has survived no competing tip |
| `--min-payout` | Smallest payout | Too low and the transaction costs more than it carries |
| `--max-batch-value` | Ceiling on one batch | A custody control; see §6 |
| `MAYA_POOL_TREASURY_PASSWORD` | Unlocks the keystore | There is deliberately no password flag |

`PoolConfig::validate` runs before anything binds. A pool that starts with an
incoherent payout policy pays wrong amounts; one that refuses to start pays
none, and the second is recoverable.

## 3. Share validation, end to end

```text
  socket ──► ML-KEM transport ──► SV2 frame ──► Session ──┬─► reply
                                                          │
                            integer checks, no hashing ───┤
                              range → job → duplicate     │
                                                          ▼
                                            bounded queue (4096)
                                                          │
                                          8 blocking workers, 1 permit each
                                                          │
                                     BlockHeader::pow_hash_at ── the chain's rule
                                                          │
                                    ┌─────────────────────┴──────────────┐
                                    ▼                                    ▼
                            append to ledger                    also met the
                            credit the miner                    network target?
                                                                         │
                                                                    submit block
```

Three properties are load-bearing:

**The pool uses the chain's rule, not a copy.** Verification goes through
`BlockHeader::pow_hash_at`, which also selects ArgonBlake or the DAG by height.
A pool with its own implementation eventually credits work the chain rejects,
and the divergence surfaces as refused blocks *after* the shares behind them
were paid for.

**Nothing expensive runs on a connection task.** One verification is 25.4 ms of
Argon2id below the DAG fork. On a connection task that sits in front of every
other message that connection has queued; at 50,000 connections it sits in front
of the runtime.

**Backpressure raises targets and never drops shares.** A full queue makes the
submitting connection wait. Sustained depth raises every channel's target, which
reduces the *arrival* rate. Dropping submissions would shed work miners already
did, and a pool that does that under load is a pool miners leave under load.

## 4. PPLNS

`N` is a quantity of work — `pplns_factor` times the work one block at the
network target represents — and the window is walked backwards from the share
that solved the block, not from the ledger tip. Shares that arrived while the
block was being submitted belong to the next window; paying them from this one
would pay them twice.

Two consequences worth stating:

- **Splitting a farm across more workers changes nothing.** Ten rigs at a tenth
  the difficulty is the same weight and the same payout. `pplns.rs` asserts it
  rather than claiming it, and the 50-worker simulation asserts the same
  property end to end.
- **The denominator is what accumulated, not `N`.** A young pool has not earned
  `N` worth of shares; dividing by `N` anyway would distribute less than the
  reward and strand the difference in the treasury.

Rounding uses the largest-remainder method, ties broken by address so the split
is a function of its input. `split` checks that the credits sum to the reward
exactly and returns an error otherwise — a payout that does not conserve value
is either minting or stranding it, and both are worse than a stopped payout
task.

## 5. Payouts, and why they cannot double-pay

```text
                    treasury refuses
  Pending ──────────────────────────────► Failed   (credits stay with miners)
     │
     │ signed against a reserved nonce
     ▼
  Signed ─── broadcast ──► Submitted ─── buried ──► Confirmed
                                ▲                │
                                └── reorg ───────┘
```

The chain requires `tx.nonce == sender.nonce` exactly
([`src/state/db.rs:418`](../src/state/db.rs)), so transactions from one account
are strictly sequential and a nonce is a slot that exists once. Three things
follow:

1. **A batch reserves its nonce and is written down before it is signed.** A
   crash between signing and broadcasting leaves a batch on disk that the next
   pass rebroadcasts *verbatim*. Signing replacement bytes for the same nonce
   would put two transactions in one slot.
2. **Creating a batch and debiting its recipients is one write.** No ordering of
   two writes is safe: one leaves a batch about to pay balances it never took,
   the other takes balances for a batch that does not exist.
3. **One batch is in flight at a time.** Pipelining means predicting nonces for
   transactions not yet accepted, and a single rejection strands every batch
   behind it.

The pool is the only spender from the treasury, so the account's nonce advancing
past a batch's reserved nonce is proof that batch landed — there is nothing else
it could have been. A reorg moves the nonce back down, which the watcher reads
as "not landed after all" and rebroadcasts into the slot that is free again.

## 6. Custody: what is bounded, and what is not

The daemon holds a spending key on a machine with a public mining port. That is
a real exposure and it is not resolved by being careful. What exists is a bound
on the damage:

- `--max-batch-value` caps what one batch can move.
- The treasury balance is checked before signing, so a drained account fails
  loudly rather than emitting transactions the mempool will reject while holding
  a nonce slot on disk.
- The keystore password is never an argument. A password on a command line is in
  shell history and in the process list.
- `maya_pool_treasury_balance` is the gauge to page on: an empty treasury stops
  every payout, and on this chain nothing refills it automatically.

**Moving the signer out of the daemon is the real fix and is not implemented.**
`Treasury::from_key` is the seam an external signer would attach to. Naming it
here rather than leaving it implied.

## 7. Storage

RocksDB, behind the `ShareLedger` trait, with an in-memory implementation for
tests. The explorer uses PostgreSQL for the opposite reason: everything it
indexes can be rebuilt from the chain, and a lost share credit is gone.

The cost is stated rather than hidden — one node, no replication, and a restore
is a file restore.

Sequences and batch ids are stored big-endian so RocksDB's byte order is numeric
order. With little-endian keys, share 256 would sort before share 2 and every
window walk would silently read the wrong shares.

Counter allocation is guarded by a mutex. RocksDB gives atomic writes, not
atomic read-modify-write; without the guard two validator threads accepting
shares at the same instant read the same sequence, and the second write
overwrites the first miner's record with the second miner's. Nothing errors, and
one miner is paid for another's work. The 50-worker simulation is what surfaced
it, and `ledger_tests.rs` now pins it.

## 8. Rig telemetry is self-reported and cannot be checked

`SubmitWorkerTelemetry` (`0x22`, in SV2's unused `SubmitSolution` slot) carries
power, temperature, and fan speed. None of it is verifiable: a rig claiming 40 W
while drawing 4,000 is indistinguishable from an efficient one.

So it is quarantined:

- exported as `maya_pool_reported_power_watts`, never as a measurement;
- rendered in a distinct colour on the dashboard, beside a sentence saying it
  cannot be verified;
- **structurally unable to reach payout arithmetic** — nothing in `telemetry.rs`
  is an input to `pplns.rs`.

Hash rate appears twice for the same reason. `measured_hashrate` is the pool's
own arithmetic over accepted share weight; `reported_hashrate` is what the rig
claimed when it opened its channel. A large gap between them is the signal an
operator wants, and averaging them would erase it.

## 9. Three listeners, three audiences

| Port | Audience | Contents |
|---|---|---|
| `--stratum-addr` | Miners | Authenticated, encrypted, the only surface that changes pool state |
| `--api-addr` | Miners and operators | Dashboard and JSON, entirely read-only |
| `--metrics-addr` | Prometheus | Off unless set; belongs inside the pod network |

Nothing on the dashboard can open a channel, move a target, or start a payout.
A dashboard that could act on the pool would be a second and much weaker door
into the parts that handle money.

## 10. Testing

```bash
cargo test -p maya-pool-service              # unit and integration
cargo test -p maya-pool-service -- --ignored # the real-hour soak tier
```

`worker_simulation.rs` runs fifty concurrent workers submitting an hour's worth
of shares — fifty rigs times sixty minutes at the pool's one-share-per-minute
target — through the real codec, the real session state machine, the real
validator, the real ledger, and the real PPLNS split. It asserts:

- no share is lost between the verdict a worker was given and the ledger;
- the reward is conserved exactly across crediting, maturation, and payout;
- a restart mid-session does not pay anyone twice;
- rigs aggregate to their miner's address rather than being paid separately;
- a share in flight when work changed is still credited, and one two jobs back
  is not.

It runs an hour's *work*, not an hour of wall time. Vardiff convergence and the
idle timer are functions of elapsed time and are proved against an injected
clock in the unit tests; the `#[ignore]`d soak tier runs the real hour at the
real pace, and is where those are observed.

The workers search on the CPU. `cuda-miner`'s `cuda` feature is off by default
so a GPU-less runner compiles a green workspace, and a test needing a GPU is a
test that never runs. The search loop is the same `hashimoto_light` the CUDA
kernel is checked against by `cuda-miner/tests/dag_parity.rs`, so what a real
GPU worker would submit is what these submit.

## 11. Deployment

[`k8s/pool/`](../k8s/pool) carries a Deployment, Services, a `ServiceMonitor`,
and a `PrometheusRule`. [`docs/grafana/pool.json`](grafana/pool.json) is the
dashboard.

The alerts worth knowing about:

| Alert | Why this signal |
|---|---|
| `MayaPoolTreasuryLow` | The pool pays from a funded account. Empty means every payout stops, and nothing refills it automatically |
| `MayaPoolValidationSaturated` | Queue depth, not connection count, is what decides whether the pool keeps up |
| `MayaPoolPayoutStuck` | A batch that stays open is a miner not being paid, and it will not clear on its own |
| `MayaPoolStaleShareRate` | Rising staleness is usually the pool's job-push latency, not the miners' rigs |
| `MayaPoolNoBlocks` | Watches share acceptance beside block discovery, so a pool that is working but unlucky does not page |
