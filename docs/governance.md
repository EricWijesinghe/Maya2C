# Governance

The chain amending its own rules, and the limits on how far that can go.

- `governance/` — the proposal lifecycle, the tally, and the bounds. Dependency-free, model-checkable.
- `src/governance/` — records, the parameter table, work credit.
- `src/state/governance_exec.rs` — execution against the block overlay.
- `src/core/governance_payload.rs` — the wire forms.

---

## The one idea to read first

**Governance must not be able to make governance unsafe.**

A system that can change every rule can change the rules protecting the process
of changing rules. The first move of a hostile majority is not to steal — it is
to remove the checks that would have let anyone react. Set the timelock to zero
and the exit window disappears; set the quorum to one and the majority stops
needing to be one.

So four values live in `governance/src/limits.rs`, are compiled into the binary,
appear in **no** `ParameterKey`, and are reachable by no transaction:

| Floor | Value | What it protects |
|---|---|---|
| `MIN_VOTING_BLOCKS` | 480 (~2h) | A vote cannot be opened and closed between two glances at an explorer |
| `MIN_TIMELOCK_BLOCKS` | 960 (~4h) | The exit window — time to sell, withdraw, fork, or decline the release |
| `MIN_QUORUM_BPS` | 1000 (10%) | A result is not an artefact of who was awake |
| `MIN_APPROVAL_BPS` | 5001 | A strict majority; anything at or below half admits a proposal *and its opposite* both passing |

Changing them is a release, which is to say it is a decision every node operator
makes individually by choosing what to run. `no_proposal_can_name_the_quorum_or_the_timelock`
in `tests/governance_tests.rs` checks the tags do not exist.

Everything that *is* governable carries a hard `[min, max]` in
`governance/src/params.rs`, checked **twice** — when the proposal is made and
again when it executes. Twice, because a release between those two moments could
tighten a range, and a value that was legal when proposed must not become law
after it stopped being legal.

---

## What self-amendment means here, precisely

A governed rule is a `u64` in consensus state. The value in force at height *h*
is a function of *h* alone, which is what makes two nodes replaying the same
chain reach the same answer.

That covers three shapes:

| Shape | Example | Mechanism |
|---|---|---|
| A scalar limit or rate | the pool protocol fee | Table entry |
| A resource bound | contract memory pages, module size | Table entry |
| **Flipping between two rules** | "DAG proof-of-work from height *H*" | Table entry; **the binary ships both rules** |

The third is where "self-amending" honestly lives, and this codebase already
worked that way before governance existed: `crypto/dag/registry.rs` has an
`activation_height`, and `core/block.rs` reads it — below that height a block's
digest is ArgonBlake, at or above it is the DAG. Governance moving such a height
is the same operation as governance moving a fee.

### What is not here, and will not be

**Native code.** There is no key whose value is a program. A node that fetched
executable code from chain state and ran it would be handing arbitrary code
execution on every machine in the network to whoever wins a vote, and no timelock
makes that acceptable.

Substrate-style runtime upgrades work because a Substrate runtime *is* sandboxed
WebAssembly. This node's consensus logic is native Rust compiled into the binary.
Shipping the new logic and letting governance choose when it activates is the
honest version of the same idea.

**The wasmtime engine.** `vm/src/config.rs` pins an exact version and says why:
the fuel schedule is not stable across releases, so two nodes on two versions can
disagree about whether a call ran out of gas. Governance moves the VM's *limits*;
moving its engine is a release.

---

## The governed parameters

| Key | Range | Default | Why the bounds |
|---|---|---|---|
| `dex.protocol_fee_bps` | 0–200 | 0 | The LP rate pays providers; the protocol's cut comes out of the same trade, so an unbounded one is an unbounded tax on every swap |
| `dex.max_fills_per_block` | 64–4096 | 1024 | Below 64 a crossed book never drains; above 4096 one block's matching is ~3 ms every node redoes |
| `dex.max_orders_per_book` | 256–65536 | 4096 | Storage and book reconstruction, which escrow does not bound |
| `oracle.max_feed_submissions_per_block` | 1–32 | 8 | Each is up to a quorum of post-quantum verifications at ~286 µs |
| `shielded.max_per_block` | 1–256 | 64 | Groth16 verification budget |
| `vm.max_memory_pages` | 16–1024 | 256 | 1 MiB to 64 MiB per call |
| `vm.max_module_bytes` | 64 KiB–2 MiB | 512 KiB | Code is stored verbatim and kept forever |
| `vm.default_gas_limit` | 1e5–1e9 | 1e7 | Too low and nothing completes; too high and one call outlasts a block |
| `governance.proposal_deposit` | 0–1e9 | 10000 | Anti-spam; the ceiling stops governance pricing out its own participants |

**No parameter can be set to zero where zero silently disables a subsystem.**
Setting a per-block ceiling to zero does not remove a subsystem — it stops it
while leaving every transaction that uses it apparently valid.
`no_parameter_can_be_voted_to_zero_where_zero_disables_a_subsystem` checks this.

### The protocol fee, specifically

`src/state/dex_exec.rs` used to say the rate was zero *"because there is no
governance process that could have decided to. A protocol fee set by whoever last
edited a constant is not a protocol fee, it is a developer helping themselves."*

That process now exists, so the sentence is answered rather than deleted. The
rate is zero until a quorum, a strict majority, and a timelock have all said
otherwise, and it can never exceed 2%.

It is read **at swap time, not baked into the pool**. The LP rate belongs to the
pool — its providers agreed to it when they deposited. The protocol rate belongs
to the chain. Storing the chain's rate inside each pool record would mean a
governance decision applied only to pools created afterwards, which is not what
"the protocol takes a cut" means.

---

## Voting power

Locked stake **plus** decaying work credit.

### Locked stake

Coin is debited from the spendable balance into a lock record. Weight that could
still be spent is weight that costs nothing to hold.

A lock must survive to the proposal's **execution**, not merely to the close of
voting. Without that, the cheapest way to decide something is to acquire weight,
vote, and be gone before the decision binds anyone who stayed. Locking again
*extends* the release and never shortens it.

### Work credit — and where it came from

This chain has no block reward. `pool-service/src/config.rs` says so in the code:
`apply_block_checked` credits no subsidy and fees burn to the fee sink. There is
no coinbase, and `BlockHeader` carries `prev_hash ‖ state_root ‖ timestamp ‖
nonce ‖ difficulty_target` — nothing identifying who mined it.

So before this work there was **no on-chain record of who did any work at all**,
and "voting power based on hash power" had no data source.

`TxKind::ClaimWork` creates one without touching the proof-of-work preimage. At
most one per block, crediting that block's difficulty to an address the *miner*
names — the miner chose the block's contents, so the miner is entitled to say who
is credited. Adding a header field instead would have changed the PoW preimage
and therefore the miner, the pool protocol, Stratum, and the CUDA kernel.

**The honest consequence:** hash-power voting is **miner-directed**, not
pool-participant-directed. A pool's miners contribute the hashes; the operator
decides who is credited. The mechanism cannot fix that — attributing work to
individual hashers needs the share data the pool holds off chain.

Credit halves every 5,760 blocks (~1 day). Voting power should reflect hash power
*now*; an all-time accumulator would let a farm that ran two years ago govern
forever. Halving is exact in integers — it is a shift — where a fractional decay
would need a rounding rule, and a rounding rule in voting weight is a bias.

### Quorum is measured against locked stake alone

Work credit decays per address at a different moment for each address, so a
chain-wide total would drift from the sum of its parts, and a quorum denominator
that is quietly wrong is worse than one that is conservatively simple.

The consequence: turnout can exceed 100% when miners vote. That only makes quorum
*easier*, never harder, and the binding threshold — approval — is unaffected.

---

## Lifecycle

```text
           ┌──────────► Cancelled       (proposer withdraws, before close)
           │
 Voting ───┼──────────► Rejected        (quorum missed, or approval missed)
           │
           └──► Queued ─┬──► Executed   (timelock elapsed, grace not)
                        └──► Expired    (grace elapsed, nobody executed)
```

Every transition is a function of height. Nothing happens because time passed; it
happens because a block at some height asked.

- **`Queued` is a state, not a delay.** A passed proposal becomes executable
  later. That gap is the exit window.
- **A passed proposal cannot be withdrawn.** It belongs to everyone who voted for
  it; letting its author retract it would make every vote conditional on the
  author's continued agreement.
- **`Expired` exists** so a proposal nobody executed does not stay executable
  forever and land years later into a chain it was never argued about.
- **Abstentions count toward quorum, not approval** — a holder who thinks a
  question should be settled but has no view can say exactly that.

### Execution is deferred, always

Finalizing and applying happen in `settle_governance`, once per block, after
every transaction is staged, in proposal-identifier order. Two reasons, the first
fatal:

1. A parameter change applied where a transaction sits would mean two
   transactions in one block executing under different rules. A node that ordered
   them differently would reach a different state.
2. Finalizing depends on the whole block's votes. A tally read halfway through is
   a tally still being written.

---

## Bounds

| Constant | Value | What it stops |
|---|---|---|
| `MAX_CHANGES_PER_PROPOSAL` | 8 | Bundling an unpopular change onto a popular one |
| `MAX_GOVERNANCE_TX_PER_BLOCK` | 32 | Every proposal opened is one the end-of-block pass visits for life |
| `MAX_AUTHORITIES`-style ceilings | see table | Each parameter's own range |
| `MAX_WEIGHT` | `u64::MAX` | Keeps every tally product far inside `u128` |
| `EXECUTION_GRACE_BLOCKS` | 100,000 | A stale change landing into a chain that moved on |

---

## What is deliberately not solved

**Whale capture.** Not solvable by mechanism on a pseudonymous chain. Quadratic
voting needs identity, and splitting a balance across addresses is free here.
What is offered is a quorum, a strict majority, a timelock, and an exit window —
not a guarantee that concentrated stake cannot decide things.

**Miner-directed work credit.** Above.

**Parameter changes to already-created pools' LP rates.** The LP rate is the
pool's, agreed by its providers. Only the protocol rate is the chain's.

**Governance over the compiled floors.** By design. A refinement would be to let
governance raise them (making itself *stricter*) while never lowering them; that
is strictly safer than today's fixed values and is a reasonable future change.

---

## Reorgs

Governance records live under `g:` and go through the same generic overlay, undo
journal, and root fold the trading (`d:`) and oracle (`o:`) subsystems use. A
reverted block restores the parameter table and every proposal's state.

It matters for the same reason it matters for pool reserves: a parameter left at
the abandoned chain's value is a rule nobody voted for on the chain that
survived.

A chain that has never governed anything has no `g:` records and therefore the
state root it would have had before this subsystem existed.
