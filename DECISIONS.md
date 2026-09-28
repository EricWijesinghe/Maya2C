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
