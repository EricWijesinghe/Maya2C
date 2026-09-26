---
title: 'Network telemetry'
editUrl: false
# GENERATED from docs/telemetry.md by scripts/ingest.mjs. Edit the source, not this.
---
## Read this before quoting any number from the dashboard

Every figure the collector serves, except difficulty, is **an unauthenticated
claim by whoever reported it**. A miner can claim any hash rate. A node can
claim any height. Nothing is verified against the chain, and closing that gap
is not a future task — there is no proof a machine can offer that it is
hashing.

This is stated at the top of `crates/telemetry/src/lib.rs`, on the dashboard page
itself, and here, because "the network has N hashes per second" is exactly the
kind of number that gets repeated without its caveat. The chain's own
difficulty is the figure that cannot be faked, and it belongs beside the
claimed rate for that reason. Where they disagree, difficulty is right.

## The parts

```text
miner ──HashMeter──> Reporter ──POST /report──> Collector ──WS──> dashboard
node  ─────────────────────────POST /report───────┘
```

| Piece | Crate | Feature |
|---|---|---|
| hash rate measurement | `crates/telemetry/src/meter.rs` | always |
| wire types, region rules | `crates/telemetry/src/{report,region,snapshot}.rs` | always |
| the reporter a miner embeds | `crates/telemetry/src/reporter.rs` | `client` |
| the collector and its HTTP surface | `crates/telemetry/src/{collector,http}.rs` | `server` |
| the browser page | `apps/dashboard/` | its own workspace |

The split matters: `wgpu-miner` takes `default-features = false,
features = ["client"]` so a miner links no web framework it never serves from —
the same discipline that keeps `stratum-v2` free of a chain dependency. The
always-on modules are also what compiles for `wasm32-unknown-unknown`, which is
how the dashboard deserializes the collector's own `Snapshot` rather than a
hand-written mirror of it. A dashboard with its own copy of that struct is a
dashboard where adding a field silently stops it being displayed.

## What the collector does about dishonest reporters

It cannot detect them. What it can do is bound their effect, and each choice
below exists for one specific failure:

| Failure | Choice |
|---|---|
| one node claims height 9,000,000 and it never comes back down | height and propagation are **medians**, not maxima |
| one reporter loops and multiplies its own hash rate | reports are keyed by reporter id and **replace** rather than accumulate |
| one `u64::MAX` claim flattens every chart to zero | hard bounds (`MAX_HASH_RATE`, `MAX_PEERS`), **rejected not clamped** |
| miners crash rather than saying goodbye, so totals only grow | a `REPORT_TTL_SECONDS` expiry, so totals can fall |
| a flood of fabricated reporter ids grows the collector's memory | `MAX_REPORTERS`, evicting expired entries first and the oldest after |

Rejecting rather than clamping is worth naming: a clamped report is a number
the reporter did not send, presented as though they had.

Hash rate is summed rather than medianed, because there is no median to take —
the network's rate is the total of what is running. That is also the one figure
a determined liar can inflate, which is why the caveat above exists.

## Location: country only, and not always that

The dashboard shows where hash rate is. It does that at country granularity and
enforces three rules in `crates/telemetry/src/region.rs`:

1. **The address is never stored.** A country code is derived at ingest and the
   address is dropped in the same function. There is no field for it in
   `Stored`, and `crates/telemetry/tests/end_to_end_tests.rs` asserts against the
   published bytes, not against the type.
2. **There are no coordinates.** Not rounded ones. A rounded coordinate is
   still a coordinate, and a UI that receives one will eventually plot it.
3. **A country with fewer than `MIN_REPORTERS` (5) reporters is folded into
   `ZZ`.** "One miner in Liechtenstein" is not aggregate data; it is an
   individual with a public dashboard pointing at them.

Folding conserves the totals — it hides which country, never how much — and it
happens when the snapshot is built rather than at ingest, because a country
crosses the threshold in both directions as miners come and go.

`??` is reporters the collector could not place. It is counted and shown rather
than dropped: a visibly large unknown bucket is a fact about the deployment,
and hiding it would bias every percentage on the page.

The country comes from a geolocating reverse proxy's header (`CF-IPCountry` by
default), never from the reporter. A self-reported country is a text field an
attacker fills in, and a map built from it looks exactly like a real one. With
`MAYA_TELEMETRY_TRUST_PROXY` unset — the default — the header is ignored
entirely and every reporter is `??`, because with no proxy in front the header
is whatever the reporter typed.

## The live feed

One broadcast channel carries snapshots to every connected browser. A task per
socket rebuilding the snapshot on a timer would make the collector's work
proportional to how many people are looking at the page.

A slow client is **lagged, not disconnected**. Snapshots are absolute rather
than incremental, so a client that missed three is fully correct after the
fourth; there is nothing to resynchronise. The handler sends the current
snapshot immediately on connect, so a browser that arrived between two reports
does not stare at an empty page.

The daemon also sweeps on a timer. Without it, a network where everybody
stopped reporting would produce no reports, therefore no snapshots, and the
dashboard would show the last healthy picture indefinitely.

## The miner side

`HashMeter` keeps 30 one-second buckets and reports the rate over the populated
ones. A running average since startup is monotone in practice — a rig that
loses half its GPUs after a day of uptime reports a number that barely moves —
and the figure the dashboard needs is one that falls when the hardware does.

`record` is one relaxed atomic add and is called once per dispatch, not once
per hash. A meter that needed a lock per batch would be measuring itself.

Telemetry can never stop a miner: `Reporter::spawn` reports from its own task,
every failure is logged at debug and dropped, and there is no retry queue. A
queue of stale rates is worse than a gap, because the TTL already treats a
missing report as unknown, which is the truth.

### What currently produces a real GPU hash rate

`wgpu-miner --benchmark`. It dispatches the validated compute path against a
real dataset in a loop and reports what the card manages:

```bash
cargo run -p maya-wgpu-miner --features gpu --release -- \
  --list
cargo run -p maya-wgpu-miner --features gpu --release -- \
  --benchmark --device 0 --benchmark-seconds 30 \
  --telemetry http://127.0.0.1:9100/report --telemetry-id rig-1
```

It is a benchmark and not mining: there is no header from a node, no target to
meet, and nothing is submitted. The hash rate is real; the mining is not, and
calling it mining would put a number on a dashboard that no chain agrees with.
`wgpu-miner` still has no work-distribution loop — the meter is wired into
`GpuMiner::mixes`, which is where the GPU work actually happens, so it is
already correct when that loop lands.

The benchmark uses `Params::TESTING`, a 4 MiB dataset. The per-dispatch cost
does not depend on how much of the dataset is resident — the shader does the
same number of page lookups either way — but an operator sizing a rig for
mainnet should read the figure as an upper bound.

`cuda-miner` is **not** wired up. Its `argon_blake_hash` is a single-hash
function with no loop of its own to meter; the loop lives in whatever calls it.
Stated here rather than left as an apparent oversight.

## Running it

```bash
MAYA_TELEMETRY_LISTEN=0.0.0.0:9100 cargo run -p maya-telemetry --release

cd dashboard && trunk serve       # proxies /api to 127.0.0.1:9100
cd dashboard && trunk build --release   # dist/ is the deployable page
```

`apps/dashboard/` is deliberately **not** a workspace member. Leptos in CSR mode
links `web-sys` and only runs on `wasm32-unknown-unknown`; adding it would put
a crate that cannot execute on the host into `cargo test --workspace`.
`sdk-wasm` is a member because its logic is target-independent and its tests
run natively — this crate is a browser UI and has no such half.

## Endpoints

| Route | Purpose |
|---|---|
| `POST /report` | a miner or node submits |
| `GET /api/snapshot` | the current aggregate, once |
| `GET /api/ws` | the same aggregate, pushed on every change |
| `GET /health` | liveness |

A malformed or implausible report is a `400`. It is the reporter's problem, and
a `500` would page whoever runs the collector for somebody else's typo.
