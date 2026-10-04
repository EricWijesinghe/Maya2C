# Decisions

One paragraph per decision, newest last, with the reason. A decision that
changes architecture, protocol, security or economics also gets an ADR in
[docs/adr/](docs/adr/); this file links it. Earlier decisions live in
ADR-001 to ADR-030 and are not repeated here.

## 2026-09-28 — One branch: `master`

Eric asked for a single branch because visitors see the default branch and
do not switch. `master` was fast-forwarded to the completed work (173
commits, no rewrite), PR #1 merged by that push, the superseded PR #2
closed, and both extra branches deleted. Work in progress may still live on
a short-lived branch (the chat session uses `feat/utopia-chat`), merged back
to `master` when it is done.

## 2026-09-28 — CI fixes go before operating-system setup

The operating protocol's priority order puts a red CI at P0. `master` had
two failing jobs (rustfmt, unsafe accounting), so those were fixed first,
on a separate worktree, before any other work in the session.

## 2026-09-28 — `cargo xtask status` reads evidence; it does not build

CLAUDE.md's "one cargo scope at a time" rule means a status command that
ran cargo would wedge `target/` whenever another build was running, which
is when people ask for status. So `status` reads the last sweep
(`reports/sweeps/latest.json`), the gap register, `features.toml` and
STATE.md, prints each figure's date and commit, and flags a sweep taken at
an older commit. `--live` adds a `cargo check` on request.

## 2026-09-28 — Clean rebuilds for sweeps, not for every "done" claim

The operating brief asked for `cargo clean` before every DONE claim. A
cold build here costs about five minutes and invalidates any other
session's build. Decision (Eric approved the recommendation): clean
rebuilds for the verification sweeps (`cargo xtask sweep --clean`);
individual DONE claims use a fresh run of the affected checks with the
output pasted.

## 2026-09-28 — Placement of Master Prompts the ladder did not name

MP09 → M3, MP10 → M5 (genesis signing at M8), MP14 → M2, MP18 → M5, MP27
and MP28 → M4 as differentiators whose activation waits for M7. MP06 → M1,
but only its ADR-016 mainnet-core parts; the rest is P6. This last one is
a proposal awaiting Eric (EMPIRE.md, Q1).

## 2026-09-28 — The gap register's three P0 rows are guards, not gaps

`crypto-pq/src/kem_suite/dual.rs:59` panics inside a `const fn`, so a bad
id fails compilation, not a running node. `node/src/crypto/keys.rs:297,301`
are `RngCore` methods that `fips204` key generation never calls; reaching
one would be a bug worth a panic. They stay in the register, which lists
rather than judges; M0 is not blocked by them.

## 2026-09-28 — Post-quantum encryption is not Maya Chat's difference

A dated search (`docs/prior-art/p2p-chat.md`) found PQ end-to-end
encryption already shipping in Signal, iMessage and SimpleX. The brief's
candidate difference does not hold; this is reported to Eric rather than
worked around (Standing Order 9). Remaining candidates, PQ authentication
and chain-account identity, need their own searches.

## 2026-09-29 — The first testnet is one seed on Oracle Always Free

Eric chose Oracle Always Free plus a home PC over a paid VPS or the
$9,000/month AWS fleet. One installer for every host
(`infra/testnet-vm/install.sh`); `infra/oracle-free` only provisions, at a
pinned commit. The home PC joins as an observer: a second validator would
halt the chain if either machine stopped. ADR-032.

## 2026-09-29 — Guides say mining earns nothing, because it does not

The owner asked for "anyone to start mining". ADR-016 makes DAG-BFT the only
production consensus, and emission is nil (ADR-029). The mining guide states
this plainly instead of inviting people to mine; the conflict is reported to
Eric in the session report.

## 2026-09-30 — Mainnet v1 launches without private transfers

An investor will fund once mainnet is live. Eric chose "mainnet v1 without
private transfers" over waiting for the full audit or launching as-is. The
shielded pool stays off at genesis and turns on at a scheduled height after
an audit. Launch gates, and three lanes for three sessions, are in
docs/mainnet-v1-plan.md.

## 2026-10-03 — Chain-bound signatures finished; txid hashes each kind's own body

ADR-036 completed. Review found the partial implementation hashed the
hybrid body into every txid, so two multisig wallets paying the same outputs
at the same nonce shared an id; the txid now hashes the authorization's own
body under a kind byte. The txid stays chain-free (dedup within one chain
needs no chain). Deferred to BACKLOG P1: wallet genesis pinning, preview
keyed on wire hashes.

## 2026-10-03 — Mainnet with the pool off is a genesis parameter (ADR-037)

`shielded_activation_height`, hashed into the genesis id only when present,
rather than a compiled constant or a cargo feature: the testnet keeps the
pool and its id, mainnet sets "never", and no operator can quietly differ.
`GenesisConfig::chain_config` became the single source of validation rules
after the audit found the CLI fork tool missing the setting.

## 2026-10-03 — Wasmtime 48.0.4 rides the ADR-036 re-genesis

Three advisories in features the VM compiles out; bumped anyway because the
pin is consensus-critical and one hard fork is cheaper than two.

## 2026-10-03 — The wallet takes the site's identity; amounts stay base units

One brand across site and wallet. Amounts are grouped base units because no
symbol or decimals are ratified — inventing them is Eric's decision, not the
UI's (with gate 6 economics).

## 2026-10-03 — A watchdog restarts the testnet stack every five minutes

The public endpoints were dark ~19 h after cloudflared exited on a network
drop and the logon script died after starting the node. A scheduled task
(`Maya2C testnet watchdog`) re-runs the idempotent start script.

## 2026-10-04 — Rejoin through quorum-attested checkpoints (ADR-038)

A validator down past the engine's 50-round window never caught up:
fetching old certificates over gossip lost replies under load, and snapshot
bootstrap landed outside the window. Every validator now signs each block
it builds; 2f+1 signatures make a checkpoint, and a node that fell behind
imports up to it, verifying the quorum against the committee it already
trusts. The serving peer is trusted for nothing. Rehearsed with real
binaries (dryrun4).

## 2026-10-04 — Quorum is n − f, not 2f + 1 (ADR-039)

2f + 1 is a safe quorum only at n = 3f + 1. Staking makes every other
size reachable, and at six validators it lets two disjoint halves
certify. n − f is identical at every size that has run (1, 4, 100), so it
needs no re-genesis. Found while checking whether outside validators could
join safely.

## 2026-10-04 — Testnet validator refuses cheap registrations (ADR-039 option 3)

One seat is one vote and costs `min_self_bond` (1,000). One absent
registration halts the one-validator testnet, and nothing recovers it. Our
validator runs with `--min-register-bond`, above what the faucet can fund,
and the project funds approved operators. This is node policy, not
consensus, and it holds only while one operator proposes every block.

## 2026-10-04 — Stake-weighted committees: engine yes, node wiring is Eric's call (ADR-040)

The engine counts stake in every threshold. Equal weights reproduce
today's behaviour, so this merges safely. Feeding real stake needs an
epoch-boundary snapshot, which is a new state record: a state-root change
needing an activation height or a new genesis. Recommendation: at mainnet
genesis.

## 2026-10-04 — Astro 5 → 7 (#21) not merged overnight

It builds, but with new warnings (the 404 entry is missing, the i18n
collection is empty). A two-major framework bump on the public site wants
a rebase and a visual check, not an unattended merge.
