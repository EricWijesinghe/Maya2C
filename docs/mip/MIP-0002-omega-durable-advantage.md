# MIP-0002: The Maya2C Omega Design: advantage that numbers do not decide

| Field | Value |
|---|---|
| Status | **Proposed**, 2026-11-02. Strategy plus four protocol mechanisms (§12, decisions O1–O4). |
| Requested by | Eric, 2026-11-02 ("Maya2C Omega Ascension Initiative") |
| Decision record | [ADR-078](../adr/ADR-078-omega-durable-advantage.md) |
| Prior art | [forkless-upgrades-and-adaptive-control.md](../prior-art/forkless-upgrades-and-adaptive-control.md), searched 2026-11-02; [chain-level-exploit-recovery.md](../prior-art/chain-level-exploit-recovery.md) |
| Builds on | the seven-point "hard to beat" plan (2026-09-28), [MIP-0001](MIP-0001-sentinel-recovery-network.md) (SRN), ADR-069 |
| Scope | After mainnet for every consensus mechanism. The test ratchet (O3) can start now, because it is CI only. |

---

## 0. The question, and the honest answer

The brief asks: *what must Maya2C become so that no future protocol can surpass
it?*

**Nothing can guarantee that.** A protocol with unlimited funding, developers
and hardware can copy any code (Maya2C's is Apache-2.0) and beat any benchmark.
A design that claims to be beyond reach invites the first counterexample, and
the claims rule forbids the language anyway (Standing Order 10).

The achievable version:
- **Compete where copying does not help.** Accumulated trust, history,
  identity, evidence and proofs belong to the network that earned them.
- **Turn a rival's advance into an upgrade.** Make every breakthrough
  something Maya2C can absorb (§6), so it becomes an input rather than a
  replacement.
- **Grow stronger with age, use and attack.** Every year, user, application
  and incident should add to an asset a newcomer starts without (§2).

This document designs those three things. Its characteristics are written as
**properties with tests**, not as superlatives.

---

## 1. Phase 1: Advantage beyond numbers

Assume competitors with 10× to 10,000× the throughput, lower fees, faster
finality and bigger blocks. Here is what still decides where value lives:

| Axis | What a user or institution actually needs | Maya2C mechanism | Status |
|---|---|---|---|
| **Survives quantum computers** | keys that stay safe for decades | ML-DSA + SLH-DSA dual signatures mandatory; NIST level 5 defaults (ADR-051); a suite registry with sunset schedules (invariant 29) | REAL, sunset planned (ADR-069 #4) |
| **Never loses funds to the protocol** | a ledger whose core cannot be exploited | conservation invariant fails any value-creating block (invariant 28); Kani-proved math crates; no `unsafe` in core execution | REAL |
| **Recovers from the exploits it cannot prevent** | rules for the bad day, written in advance | SRN: scoped quarantine, forward-only recovery (MIP-0001) | Accepted, after mainnet |
| **Protects people from themselves and from scams** | clear signing, delays, guardians, duress keys | clear-sign describe (#271), guardian recovery (ADR-056), Scam Shield (ADR-069 #1–2) | partly REAL |
| **Tells the truth** | numbers anyone can reproduce | every benchmark carries commit, hardware and command; claims-check in CI; MISSING instead of estimates | REAL (process) |
| **Is verifiable without trust** | run it yourself, check every claim | reproducible builds (ADR-054), conformance vectors, a spec with a second-client path | partly REAL |
| **One identity across every app** | contacts, reputation and recovery that follow you | PQ identity (DID), chat, wallet and Hub on one key | partly REAL |
| **Neutral** | no insider allocation to distrust | no premine; rewards by work (ADR-044) | decided |

None of these rows is a throughput number. A faster chain without them is a
faster place to lose money.

---

## 2. Phase 2: Moats that compound

A moat counts only if it **grows with time, users, applications or
validators**, and only if a fork of the code cannot take it. Each has a
measurable indicator, so "is the moat growing?" has an answer.

| Moat | Grows with | Why a fork cannot take it | Indicator (to publish) |
|---|---|---|---|
| **Track record** (Lindy) | years | a fork resets its own history to zero | days since genesis with no halt, no lost funds, no rewind |
| **Shared immunity** | incidents survived | each attack adds a replay test, an evidence record and a detector; a fork starts with none of the operational history | exploit replays, SRN corpus size, mean time to quarantine |
| **Identity and reputation graph** | users | your contacts, attestations and recovery guardians live on this chain's state; leaving means starting over | active identities, attestations, guardian links |
| **Proof corpus** | releases | Kani harnesses, conformance vectors, spec rules: each release adds and none are removed (O3) | harness and vector count, never decreasing |
| **Crypto-agility as a standard** | adopters | the suite registry and vectors become what other projects implement | external adopters of the registry |
| **Decentralization** | validators | reward tilt toward small operators (ADR-069 #3) raises the Nakamoto coefficient over time | Nakamoto coefficient, HHI per epoch |
| **Developer surface** | applications | SDKs, docs, examples and audited modules that apps depend on | apps, SDK downloads |
| **Quantum Harbor holdings** | quantum risk awareness | assets committed to PQ vaults before a quantum break are claimable only here | value committed |

These are network effects in the plain sense: each new participant makes the
chain more useful to the next. No formula like Metcalfe's law is asserted here,
because none has been measured for this chain.

---

## 3. Phase 3: Protocol intelligence from first principles

The brief rejects external ML frameworks and asks for intelligence native to
consensus. Invariant 20 (no floats in consensus) already demands most of that.
The design is **verifiable cybernetics**: the protocol steers itself with
integer feedback controllers whose behaviour can be proved.

### 3.1 The cybernetic control plane (decision O2)

Every adaptive parameter in consensus is a **controller** with five written
parts:

| Part | Meaning | Example (base fee) |
|---|---|---|
| Measured variable | an integer read from committed state | block fullness in bytes |
| Setpoint | a governed target, inside compiled bounds | 50% full |
| Actuator | the parameter it moves | base fee |
| Law | integer PI control with anti-windup, step-clamped per block | rise ≤ 1/D per block (ADR-076) |
| Proof | bounded output and bounded step, as a Kani harness or property test | the base-fee clamp tests |

The control law, in integers only:

```
e_t      = setpoint − measured_t                      (i128, saturating)
I_t      = clamp(I_{t-1} + e_t, −I_max, I_max)          (anti-windup)
Δu_t     = clamp((Kp·e_t + Ki·I_t) / SCALE, −Δmax, Δmax)
u_t      = clamp(u_{t-1} + Δu_t, u_min, u_max)
```

Gains `Kp`, `Ki`, `SCALE`, `Δmax` and the bounds are governed numbers
(invariant 13: never programs). The stability argument is the clamp: `u` stays
within `[u_min, u_max]` and moves at most `Δmax` per block whatever the input.
That is provable, which an ML model's behaviour is not.

Controllers that exist or are designed:

| Controller | Measured | Actuator | Status |
|---|---|---|---|
| Base fee | fullness | fee | REAL (ADR-076 clamp) |
| Neural gas | fee-market features | fee | RESEARCH (quantized integer network; `bins/neural-gas-trainer`) |
| Decentralization tilt | Nakamoto coefficient, HHI | reward split | planned (ADR-069 #3) |
| SRN thresholds | false-positive rate of quarantines | `S1`–`S3` | planned (MIP-0001) |
| Block byte cap | apply time per MiB against the interval | cap | candidate (measured apply cost: `reports/bench/2026-11-01-apply-per-mib.md`) |

### 3.2 Swarm and evolutionary search: evolution off-chain, selection on-chain

- **Search off chain.** Evolutionary search (mutation and selection over
  parameter vectors) runs in the deterministic econ simulator, which calls
  the real fee-market and DEX code. Anyone can rerun a search from its seed and
  get the same result.
- **Select on chain.** Governance adopts a parameter set only inside compiled
  bounds, with the simulator run's hash attached as evidence.
- The chain never runs the search. It runs the winner's numbers.

### 3.3 Information theory and network science in consensus signals

- **Concentration:** the Herfindahl–Hirschman index (HHI) of stake and of block
  authorship, `Σ s_i²` in integers. This is cheap, deterministic and already
  close to `staking/concentration.rs`.
- **Surprise:** an outflow or price move scored by how far it sits outside a
  trailing integer histogram. This is SRN's deterministic anomaly signal.
- **Cellular automata:** studied for this design, and no consensus use found
  that beats the controllers above. Recorded so the question is not reopened
  without a new reason.

### 3.4 Where ML stays

Off-chain sentinels and the learning layer (MIP-0001 §9) may use any model.
Their output is advisory. That boundary is the point: anything that steers
consensus can be re-executed by every node.

---

## 4. Phase 4: A self-evolving protocol, three speeds (decision O1)

| Speed | What changes | Who decides | Fork? |
|---|---|---|---|
| **Fast (every block)** | controller outputs (§3.1) | the control law | no |
| **Medium (weeks)** | setpoints, gains, thresholds within compiled bounds | governance, timelocked | no |
| **Slow (months to years)** | code: new transaction kinds, new suites, a better consensus engine | **pre-shipped, vote-activated**: the code ships dormant in a node release; an on-chain vote fixes its activation height | a coordinated upgrade, with no chain split for nodes that upgraded |

The slow speed is close to Tezos's self-amendment and Polkadot's runtime
upgrades, with one deliberate difference. **Maya2C never runs code fetched
from chain state** (invariant 13). The vote selects *when* code that operators
already installed and reviewed switches on, never *what* code runs. That costs
some convenience; it removes the governance-installs-a-backdoor attack class.

Crypto-agility is part of the slow speed:
- a new signature or KEM suite enters the registry dormant;
- a vote activates it;
- the sunset schedule (ADR-069 #4) retires the old suite with a migration
  window.

---

## 5. Phase 5: The immune system, formally

Model the chain as an organism with five systems. Each maps to things that
exist or are designed, and each has a formal property.

| System | Maya2C | Formal property |
|---|---|---|
| **DNA** (what never changes) | genesis hash; spec; the invariants (`docs/invariants.md`); conservation; determinism; dual PQ signatures | `I = {I_1 … I_n}` holds in every committed state: `∀ s ∈ Reach(genesis): ∀ i: I_i(s)` |
| **Memory** | exploit replays, conformance vectors, Kani harnesses, SRN evidence corpus | a ratchet: `T_{t+1} ⊇ T_t` (tests are added, never removed) (O3) |
| **Immunity** | invariant guard (invariant 28); SRN levels T1–T3 | conservation violation ⇒ block invalid; anomaly ⇒ cell quarantine, no transfer stopped |
| **Healing** | quarantine → probation → normal; forward restitution | every restriction expires unless renewed by a stronger act (SRN P3); no finalized block reverted (SRN P7) |
| **Evolution** | the three speeds (§4) | controller outputs are bounded: `u_t ∈ [u_min, u_max]`, `|u_t − u_{t−1}| ≤ Δmax` |

**Immune learning as a loop:**

```
incident → evidence record → replay test (memory, ratchet) → deterministic rule (immunity)
        → advisory to operators → detector update (off-chain) → next incident caught earlier
```

**A measurable claim this model makes:** mean time to quarantine for a known
attack class falls with each recorded incident of that class. It is MISSING
until SRN runs; the model says what to measure.

---

## 6. Phase 6: Hard to obsolete, by absorbing what would replace it

| Why technologies die | Maya2C's absorption mechanism |
|---|---|
| Its cryptography breaks | the suite registry, dormant suites and the sunset schedule; Quantum Harbor for assets on other chains |
| A better consensus appears | consensus sits behind a spec with conformance vectors; a new engine is a slow-speed upgrade, tested on the attacknet first |
| A better VM appears | the multi-VM plan (ROADMAP "Deferred modules" 7) |
| Better economics appear | controllers and bounded governance (§3, §4) |
| A competitor is socially preferred | neutrality (no premine), honest claims, transparent incident rules (SRN) |
| A bigger ecosystem pulls users away | interop: HTLC swaps, trustless interop, Quantum Harbor; one identity users can take into every app |
| The founder or core team goes away | open source, spec, second client (`docs/SECOND_CLIENT.md`), documented runbooks; still a real risk today (§9) |

**The absorption pipeline:**
1. Watch: `docs/CRYPTO_WATCH.md` and a monthly research scan.
2. Record: `docs/prior-art/` entries.
3. Prototype: a RESEARCH crate with tests.
4. Activate: an activation height, through the slow speed.

A breakthrough published anywhere becomes a RESEARCH entry here within one
cycle, which is how a threat becomes an input.

---

## 7. Phase 7: A unified network for more than money

| Value type | What the chain records or enforces | Status |
|---|---|---|
| Money | 256-bit amounts, 24 decimals, PQ signatures | REAL |
| Trust | attestations, guardians, SRN rules, threat intel | partly REAL |
| Security | Quantum Harbor, sealed validator keys, invariant guard | partly REAL |
| Knowledge | data anchors (`iot-anchor`), evidence corpus, content-addressed archives | RESEARCH |
| Computation | VM; verified AI-work claims (ADR-069 #9) | VM REAL; AI work planned |
| Identity | PQ DIDs, recovery | partly REAL |
| Infrastructure | RWA records, ISO 20022 bridge, payments | RESEARCH |

The unifying mechanism is that **every type is a state record under one state
root, with one PQ identity able to sign for all of them**. One identity holding
value, reputation, credentials and compute claims is worth more on one chain
than spread over five. That is the network effect in §2.

---

## 8. Phase 8: Horizons (speculative; a direction, not a plan)

| Horizon | Architecture |
|---|---|
| **10 years** | Decentralized committee past the independence floor; SRN active; Scam Shield and identity standard in wallets; at least one cryptographic suite rotation rehearsed end to end; a second independent client; Quantum Harbor holding real assets |
| **25 years** | Expect at least one PQ suite to be weakened and retired through the sunset schedule. A formally verified core spec (the Kani corpus grown into whole-transition proofs). Stateless validation as the default, so a phone validates. |
| **50 years** | Cryptography beyond today's lattice and hash families, entered as new suites. A century data plan: archives in open formats with independent mirrors, so state history outlives any company. |
| **100 years** | The protocol as a standard implemented by independent teams. The genesis hash as the one continuous identity, carried through every suite and engine change. |

Nothing past ten years is engineering; it is the direction the three-speed
design keeps open.

---

## 9. Phase 9: Absolute red team

A rival has unlimited funding, developers, hardware and marketing, more
throughput, and better AI.

| Attack | Does it work today? | Turn it into strength |
|---|---|---|
| Fork the code and outspend us | the code yes; the history, identity graph and evidence corpus no | publish moat indicators (§2) so the gap is visible |
| Beat every benchmark | yes | do not compete on that axis; publish honest numbers with method |
| Out-market us | yes, in the short run | verifiable claims; claims-check; independent reproductions |
| Better AI | yes, off chain | our steering is deterministic and provable; AI stays an advisor to us too |
| Hire our talent / we are one developer | **real weakness today** (bus factor of one) | spec, runbooks, second client, grants; it improves only with people (no document fixes it) |
| Launch is on one PC | **real weakness today** (ADR-047) | disclosed; recovery off until the independence floor; reward tilt toward small validators |
| No independent audit yet | **real weakness today** | grant-funded audits and a bug bounty after mainnet; Kani and fuzzing until then |
| Regulatory pressure | possible | no premine, no token sale without a lawyer (standing rule) |
| A quantum break of a suite | prepared | dual signatures plus agility: one suite breaking leaves the other |
| Governance capture | possible after decentralization | bounds, timelocks, holder veto, no code from state (invariant 13) |

Three weaknesses cannot be turned into strengths by design alone: the bus
factor, the single-PC launch and the missing audit. Each has a path, and each
path needs time, people or money.

---

## 10. Phase 10: The Omega design in one page

Maya2C should be:
1. **Durable:** post-quantum by default, with suites that rotate before they
   break.
2. **Safe by construction:** conservation enforced on every block; core math
   proved; no `unsafe` in execution.
3. **Resilient:** scoped containment and rule-bound, forward-only recovery
   (MIP-0001).
4. **Self-steering, provably:** integer controllers with bounded outputs.
   Evolution is searched off chain and selected on chain.
5. **Evolving without code from chain state:** three speeds, with vote-activated,
   pre-shipped upgrades.
6. **Compounding:** track record, immunity, identity, proofs and
   decentralization, each with a published indicator.
7. **Honest:** every number reproducible, every claim backed by a dated record.

**What this design does not claim:** that it cannot be beaten. It claims that
bigger numbers, faster hardware, lower fees and outside AI do not by themselves
beat it, because none of them buys the seven properties above.

---

## 11. New disciplines this names

- **Protocol immunology:** the study of a protocol's DNA (invariants), memory
  (the test ratchet), immunity (guards), healing (quarantine and restitution)
  and evolution (bounded change), with the properties of §5.
- **Verifiable cybernetics:** adaptive control where every controller has an
  integer law and a machine-checked bound (§3.1).

These are names for this project's practice. No claim is made that nobody has
worked on these ideas; the prior-art record lists control-theoretic tokenomics
research.

---

## 12. Decisions (ADR-078)

| # | Decision | Recommendation |
|---|---|---|
| O1 | Three-speed evolution, with code upgrades pre-shipped and vote-activated; never code from chain state | yes, after mainnet |
| O2 | Every adaptive consensus parameter is a bounded integer controller with a proof (§3.1) | yes; applies to new controllers, existing ones retrofitted when touched |
| O3 | The test ratchet: Kani harnesses, conformance vectors and exploit replays are never removed, enforced in CI the way the lint ratchet is | yes, **now** (CI only, no launch-binary change) |
| O4 | Publish the moat indicators (§2) on the status page, measured, never estimated | yes, after mainnet |

## 13. Roadmap

| Step | Work | Done when |
|---|---|---|
| Ω1 | O3 test ratchet in CI | a removed harness or vector fails CI |
| Ω2 | Moat indicators computed from chain data | the status page shows them, each with its query |
| Ω3 | Controller framework (O2) as a library in `ledger-math` style, dependency-free for Kani | one existing controller (base fee) expressed in it with an unchanged state root |
| Ω4 | Vote-activated upgrades (O1): an activation-height registry set by governance for dormant code | a dormant feature switched on by vote on the attacknet |
| Ω5 | Evolution-off-chain pipeline: seeded searches in `econ`, results attached to governance proposals | a search rerun from its seed reproduces the proposal |
