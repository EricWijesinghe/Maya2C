# 2026-10-11 — container staging net + Kali RPC stress test

Session goal (Eric): finish at least one gate or site update today; use the
full PC (RTX 5070, 24 logical cores, 31 GB) and many virtual hosts rather than
one; GUI/site upgrade; Kali WSL for a security stress test. Oracle Cloud is
deferred, so this models a multi-host network locally.

All commands and their real output are below. Nothing here is gate 4: this is
**one physical machine**. See `infra/stagenet/README.md`.

## 1. CI unblocked (toward gate 7)

PR #108 clippy was red: `clippy::unwrap_used` is denied in `--lib --bins`, and
Nemotron's `attacknet-controller` had 12 `unwrap()`s in shipped code.

- Fix: `f5069d97` — missing bootnode ports now error, clock-before-epoch maps
  to 0, empty height lists return "not yet" instead of panicking.
- `cargo clippy -p attacknet-controller --lib --bins -- -D clippy::unwrap_used`:
  clean. Lint ratchet: `lint_debt: 794 diagnostics (baseline 794) / ok`.
- PR #108 after the fix: 27 pass, 3 pending, 0 fail (was 1 fail).

## 2. Dependency patches (Dependabot)

`f56dcb63`. Patched the alerts that were real:

- JS SDK: vitest 3 → 4.1.11 (pulls patched tinypool and `@vitest/mocker`),
  `source-map-js` patched. `npm test`: 19 passed, 2 skipped.
- Docs site: `postcss-selector-parser` pinned ≥ 7.1.6 via an npm override.
  Rebuilt; `_astro/*.css` **byte-identical** (md5 diff empty). CSP check:
  72 pages, every inline script hashed.
- `rustls` 0.23.45 in `fuzz/` and `apps/wallet-gui/src-tauri/`. wallet-gui
  `cargo check` clean (16m 43s).

## 3. Site: mainnet gates dial (site update, done)

`56f04564`. The home page showed only the ADR-042 launch ladder, which still
read "one validator for now". Added `MainnetDial.astro` + `mainnet-gates.ts`
(mirrors `docs/mainnet-v1-plan.md`): an interactive ten-segment dial, one card
per gate with its evidence link, lit by real state (6 met / 10). Fixed the
ladder's stale "one validator" line to "four validators, committee of 4".

Rendered headless at 1440, 1100 (light theme) and 390 (phone): no console
errors, no horizontal overflow. `npm run build` + `check-csp` clean.

## 4. Container staging net (many hosts from one PC)

Built Linux `maya2c-node` + `l1-wallet` in Ubuntu WSL from the PR branch
(`release` profile, 9m 22s). `infra/stagenet/gen.sh` then produced a 7-validator
network where **each validator runs in its own container**, with its own IP on
a `172.30.0.0/24` bridge and its own `tc netem` WAN link:

```
v0: netem delay 15ms 3ms
v2: netem delay 80ms 15ms loss 0.5%
v4: netem delay 120ms 20ms loss 1%
(cycled across the 7)
```

`docker compose up -d` → 7 containers up. Consensus from a cold start:

```
t+20s  all 7: epoch 0, committee 7, follower false, committed_round advancing
t+40s  all 7: epoch 1, committee 7, follower false  (epoch boundary crossed)
```

The stake-weighted committee reshuffles seats each epoch (validator index
changes per node), re-elects all seven, and never halts. Verified across
epochs 0→5.

**Fault tolerance under a lossy link (observed).** The worst link, v4
(120 ms + 1% loss), is periodically jailed for downtime; the committee drops to
6 and the other six carry the chain, then v4 is re-elected. Snapshot at epoch 5:

```
v4: {"epoch":5,"round":0,"committed_round":76,"validator":null,"committee":6,"follower":false}
v0,v1,v2,v3,v5,v6: committee 6, follower false, committed_round 74–76 advancing
```

NOT VERIFIED: whether an earlier 7→6 drop during the RPC flood was caused by the
flood starving the consensus task or by netem loss. It self-healed to 7 either
way; the node handles RPC on separate tasks and the limiter rejects early, so
loss is the likelier cause, but this was not isolated.

Operational note: WSL shuts a distro down shortly after its last session ends,
which SIGKILLs dockerd (containers exit 255 with no panic). A `sleep 86400`
keepalive holds the distro for the soak.

## 5. Kali RPC stress test (security)

From Kali WSL (`wrk`, `slowhttptest`, `nmap` installed; downloads pre-approved),
targeting the staging net only — never the live public testnet. One node's RPC
was published to the WSL host via `socat` (closed again after the test).

```
wrk -t4 -c64 -d30s  →  124,876 requests, 4,018 req/s,
                       123,274 non-2xx, p50 15.3ms (= v0's netem delay)
```

At first this looked like a failure. It is not: it is the **per-IP RPC rate
limiter working as designed**. Isolation and root cause:

- Direct-to-container (no socat): same ~98.6% non-2xx — not a socat artifact.
- A 100-request burst on fresh connections (curl, `-P64`): **100/100 → 200**.
- wrk knee sweep (c=8,16,32,48): OK count **constant at ~500–508 per 10s**,
  independent of concurrency — the signature of a rate cap, not a concurrency
  cap. ~500 ≈ burst(100) + 50/s × 10s.
- Two sequential requests on one keep-alive connection: both 200.

Confirmed in code: `crates/node/src/rpc/limit.rs` (token bucket, per client IP,
`MAX_TRACKED` 65,536), enforced at `crates/node/src/rpc/server.rs:768`
(`too_many_requests()` → HTTP 429), defaults `rate_limit_per_second = 50`,
`rate_limit_burst = 100` (`crates/node/src/config.rs:150`).

So a single-IP flood of ~4,000 req/s against the RPC is absorbed: 50 req/s are
served, the rest get 429, and **consensus kept producing throughout**. This is
the correct DoS behaviour. Caveat for the record: the bucket is per source IP,
so a distributed flood gets one bucket per IP (bounded by `MAX_TRACKED`); that
is a design choice, not a regression, and matches the limiter's own docs.

## What this does and does not move

- Gate 7 (CI green): clippy fixed, deps patched; 0 failing checks, 3 pending.
- Gates 8/10 (rejoin, no halt): reinforced — the chain survives a flaky node
  being jailed and re-elected, on realistic WAN links, across 5 epochs.
- RPC DoS resilience: stress-tested from Kali and verified against the code.
- Site: the mainnet-gates dial is live in the build.
- NOT gate 4. Gate 4 needs separate machines and operators (Oracle, Eric).

## Commits
`f5069d97` clippy fix · `f56dcb63` deps + stagenet · `56f04564` site dial ·
this report to follow.
