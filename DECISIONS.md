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

Eric chose "mainnet v1 without
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

## 2026-10-05 — Launch without outside people (ADR-042, Eric)

Eric chose all three recommended options:
- **Gate 5:** a foundation validator set, four project-run validators on four
  machines (PC + Oracle Always Free), stated publicly as foundation-operated.
- **Gate 6:** a self-run daily attacknet for four weeks plus a credit-only
  break-it programme.
- **Gate 7:** a pre-audit package, free audit programmes applied for, and
  mainnet labelled "external audit pending".

Why: every gate waiting on outside parties would have blocked launch
indefinitely. Each gap is written on the site, never relabelled away.

## 2026-10-10: Nemotron-week work kept only where evidence backs it

Eric used another model for a week while Claude was out of quota. Kept: the
lint fixes and the property tests, once corrected. Not kept:
- the lab PowerShell scripts, none of which parsed (quarantined outside the
  repo);
- `compact_epoch_logs`, an unwired rewrite of the anti-equivocation log that
  drops undecodable records.
The controller's unimplemented faults now fail instead of reporting PASS.

Why: a PASS with no fault behind it is worse than no test (Standing Order 1).

## 2026-10-10: validators serve snapshots; catch-up picks a live source (ADR-043)

The live seed, 13 epochs behind, had no way back. Validators now snapshot by
default, and `--catch-up-from` is repeatable.

Why: checkpoints alone cannot bring back a node beyond the 8-epoch retention.
Validators prune blocks, so a snapshot is the only thing that can.

## 2026-10-10: `Vm::validate` stays compile + imports

The new tests expected `validate` to reject a missing `invoke` or `memory`
export and more than 256 pages. It does not; `execute` refuses all of these,
and the tests now pin that.

Why: tightening `validate` changes which deploys are valid. That is a
consensus change, and it would need an activation height.

## 2026-10-04 (recorded 2026-10-10): gate 6 economics is fees-only at launch

Eric delegated the economics to Claude on 2026-10-04. The choice: no
emission. Validators earn tips, and the base fee is burned (ADR-029). A reward
schedule can be added only through a reviewed upgrade.

Why: with nothing minted, nothing is created that an unaudited bug could
inflate.

This was decided in conversation and never written down here, so the gate
table showed it as open until today.

## 2026-10-11 — Local multi-container staging net, explicitly not gate 4

With Oracle Cloud deferred, `infra/stagenet/` models a multi-host network on
one PC: one validator per Docker container, a private bridge giving each its
own IP, and a `tc netem` link per node (15–120 ms, up to 1% loss). This
exercises peer discovery over distinct addresses, latency and loss, and
staking re-election across epochs — none of which a single-process localnet
shows. It is **not** gate 4 and may never be described as independent: gate 4
requires separate machines and separate operators, and a shared host shares a
kernel, clock and scheduler. The README states this; results are recorded as
"staging net (N containers, one host)". Chosen because it is zero-cost, uses
the real release binary, and surfaces WAN-shaped behaviour before the Oracle
machines exist.

## 2026-10-11 — RPC rate limiter confirmed as the DoS defense, not a bug

A Kali `wrk` flood (~4,000 req/s, single IP) against a staging node's RPC
returned ~98% non-2xx. This is the per-IP token-bucket limiter
(`crates/node/src/rpc/limit.rs`, 50 req/s + burst 100, HTTP 429 at
`server.rs:768`) working as designed: ~50 req/s served, the rest rejected,
consensus unaffected. No code change — the finding is that the defense holds
under a real flood. Distributed floods get one bucket per IP, bounded by
`MAX_TRACKED` (65,536); left as-is, matching the limiter's documented design.

## 2026-10-12 — Mining shares past the epoch budget are refused; retarget on time to exhaustion

The first devnet run showed unpaid shares taking block slots and nonces. They
are now refused (consensus). Because overshoot no longer shows in `shares`,
the retarget extrapolates demand from `spent_after`, the block at which the
budget ran out. The record encoding is v2. Details are in ADR-044, "Changes from the first
devnet run". Parameter consequence: `max_shares_per_block × epoch_blocks ≥
4 × target_shares` for the full ×4 hardening.

## 2026-10-12 — The Hub on phones watches, does not mine

Phone mining yields next to no hashes for battery and heat. The phone Hub
shows the network, the universe and payouts, and runs no miner. Platform order:
Windows, Linux and Android now; macOS and iOS when Eric has a Mac.

## 2026-10-12 — Repository lockdown: suggestions in, control stays with the owner

Eric: outside contributions are suggestions; nothing reaches `master` until
Claude has inspected and explained it and Eric has said yes, and no access is
granted to anyone else. Enforced on GitHub, not by convention:
ruleset `master: owner-approved changes only` (PR required, six required
checks — reality ledger, spec conformance, rustfmt, clippy, nextest (core),
cargo-deny — no force-push or deletion; the admin may bypass only through a
PR, so even the owner's token cannot push directly); ruleset
`release tags: owner only, immutable`; fork PR workflows wait for approval
from every outside contributor; Actions limited to GitHub-owned plus seven
named owners and **SHA pinning required** (all 108 `uses:` pinned, PR #110).
Zero webhooks is deliberate: nothing needs repo events sent off-site.
Openness: Discussions, private vulnerability reporting, issue forms that
route security reports privately, CONTRIBUTING / CODE_OF_CONDUCT (PR #109).
Releases: only the newest stays published, so known-vulnerable binaries are
not left downloadable; testnet.1 and testnet.2 were deleted with their tags
(commits remain in history: 930e7411, 9917db56). CodeQL: 862 alerts triaged
one by one and dismissed with reasons (757 test fixtures, 105 false
positives, including the two zero-nonce seals — safe, fresh ML-KEM secret per
seal); the three real JS findings were fixed.

## 2026-10-12 — Foundation validator set on one PC (ADR-047)

Eric chose the "PC launch" on 2026-10-11 and confirmed on 2026-10-12 that it
should be recorded and gate 5 closed on it. ADR-047 supersedes ADR-042's
four-machine gate 5: four project-run validators on Eric's PC, the same
sentence about it everywhere it is described, and the host's own failure
modes closed first (`infra/foundation-pc/harden-host.ps1`). Investigating
it found that the watchdog task ran only in Eric's interactive session, so
after an unattended restart no validator would start until he logged in.
`-Apply` re-registers it to run at boot (S4U), and `-SelfTest` proves it runs
in session 0. Residual risks, written into the ADR: no machine independence;
the PC is on the Insider Dev channel, which cannot be left without
reinstalling; and the reboot drill waits for gate 3's soak, because a
deliberate restart would reset the soak clock.
## 2026-10-12 — Helpers join with one line; the installers restart and recover by themselves

Eric: friends want to help and are not technical, so their comfort comes
first. One pasted line per OS; a helper command for every later step; keys
never leave the machine and uninstall keeps them. The installers are tested
on real macOS, Windows and Linux runners (`join-scripts.yml`), not only here.
Joining nodes fetch 4 blocks at once (validation still in order, one by one);
a bootstrap interrupted before its snapshot is discarded and redone; Windows
restarts an exited node itself. The release lookup reads the releases feed,
because api.github.com's 60/hour limit failed the macOS run.

## 2026-10-12 — In-app updates are off until the owner holds the signing key

Whoever holds the update-signing key can push code to every installed app,
so the owner generates it, never Claude. Builds stay unsigned and the
updater stays off until the secret and `apps/updater-pubkey.txt` exist.

## 2026-10-13 — Chat stickers: files named by their hash, packs shared person to person

Eric: Telegram-level stickers are a main reason people love Telegram. A
sticker rides inside the existing end-to-end encryption as a framed message
(one file, at most 120 KB: the envelope's CBOR doubles ciphertext, so that is what fits the relay's 256 KiB in the worst case, measured) named by its
BLAKE3 hash, which the receiver re-checks; the format is read from the bytes.
No sticker server: a pack travels from the person who has it, on request,
and only packs they made or installed are ever sent. Formats: Lottie (.json,
Telegram .tgs), WebM, WebP, PNG/APNG, GIF, SVG (drawn only as <img>). The
built-in pack is original art; no third-party pack ships with the app.

## 2026-10-13 — Every amount is 256-bit (ADR-046 amendment)

Balances were widened to U256 but transaction amounts stayed u64, which at 18
decimals caps a transaction at ~18.4 coins. Eric: "Widen every amount to
256-bit" (option 1 of two; option 2 was 9 decimals). Wire-format change under
invariant 31: spec, vectors and spec-ref follow. The proved crates share one
dependency-free 256-bit type, `maya-amount` (ADR-048), rather than five copies.
Done 2026-10-13 (`feat/u256-supply`): every value path is 256-bit; the values
deliberately left at 64 bits, and why, are tabled in ADR-048.

## 2026-10-18 — Overnight run decisions

- **Validator keys sealed at rest (ADR-049).** This is a new format instead of reusing the 32-byte-seed keystore, because node keys are expanded ML-DSA-65 secrets. A production build refuses plaintext; devnets warn so the live testnet survives the upgrade.
- **No backdoor (ADR-050, proposed).** "Full control" for Eric means operator control plus the existing bounded council pause at 2-of-3. It never means a master key over anyone's funds. Claude holds no keys.
- **Level 5 ordered by cost (ADR-051, proposed):**
  - now: transport ML-KEM-1024 and an opt-in wallet suite;
  - at mainnet genesis: validator keys at ML-DSA-87;
  - not now: the default account suite.
- **Funding links are left empty** until Eric creates the accounts; inventing them would be dishonest.
- **The release SBOM uses cargo-cyclonedx** through the allowed taiki-e action, because the action allow-list does not admit anchore.

## 2026-10-18 — Ticker, council, wallet units (Eric)

- **Ticker MAYA.** Display only: amounts stay 256-bit base units, and 1 MAYA = 10^18 base units (ADR-046). Recorded in `chain/maya2c-testnet.json` and as `TICKER` in the wallet core.
- **Emergency council 2 of 3 (ADR-050 accepted):** this PC, the MacBook, and an offline backup. It pauses one module for a bounded time and never moves funds.
- **Wallet send form uses whole coins (#135 merged).** Payment requests and QR codes stay in base units.


## 2026-10-19 — "All decisions are approved, take the best recommended" (Eric)

- **Council approvals are bound to their chain (ADR-050 finding resolved).** The signed message gains the genesis tag and the domain moves to `security-council/v2`. No live chain had a council, so nothing on chain changes.
- **Council settings:**
  - `max_pause_blocks` is 6 hours of blocks, computed at the ceremony from the mainnet block time;
  - the third key is kept offline, as the sealed file plus a paper passphrase in two places.
- **ADR-055 (Quantum Harbor binding) accepted.** It is built after the launch gates, with an `AnchorAttestation` anchor, so there is no new transaction kind. `k256` goes into `quantum-harbor` only, after an audit check.
- **ADR-056 (guardian recovery) accepted.** It goes on the testnet first, behind an activation height. The default delay is 7 days, with a minimum of one epoch. Vaults stay separate.
- **The soak clock resets: the new earliest pass is 2026-10-26 21:28 KST.** Superseded the same night: the testnet moved to the Oracle VM (Eric approved 2026-10-20), and the VM chain starts at block 1, 2026-10-20 00:04:11 KST, so **the earliest pass is 2026-10-27 00:04 KST**.
  - What happened: the PC went to battery at 18:19:04 KST, and a forced suspend at 18:42 stopped every node.
  - Measured on chain: no block was produced between 18:42:40 and 21:28:31 KST (2 h 45 min 51 s, before height 129,307).
  - A 7-day soak with a 2 h 46 min stop is not a 7-day soak.
  - Fix: battery standby and hibernate are now set to "never". The PC still needs to stay on mains power; only Eric can do that.

## 2026-10-25 — Launch without an external audit (Eric)

- **Decision (Eric):** no external audit, no waiting on the four-week gates. Code and build are finished this week; real-world use is the point. "Nothing is perfect."
- **Gates waived by the owner, not passed** (`cargo xtask go-no-go` keeps printing them as FAIL; that is correct):
  - external audit, zero open critical/high;
  - attacknet of four weeks or more (a self-run attacknet of about a week is planned instead);
  - incentivized testnet meeting SLOs for four weeks;
  - external validators across providers and countries (the launch set is hosts Eric controls, four if a host is found, otherwise three);
  - genesis frozen in a signed file is **not** waived: it is Eric's step at genesis.
- **What the launch must say publicly:** pre-audit, self-run reviews only, validators run by one operator; never "audited" or "independent" (claims-check, CLAUDE.md order 10).
- **Kept regardless:** the 20% per-operator cap and 7-or-more validators (`cargo xtask genesis-check`), `--trusted-block` on bootstrap, ADR-044 block rewards, ADR-050 council, ADR-051 option 3 keys, wasmtime bump.
- **Still needs `APPROVED` from Eric:** each deploy, the re-genesis, key ceremonies, hosts that cost money, and mainnet genesis itself.

## 2026-10-26 — Audit findings: what was fixed, what waits (Claude, for Eric)

- **Fixed without a policy question:** identity revocation index wrap, vault window leak in
  block selection, storage errors in selection, preview key and multisig approvals,
  `import_headers` refusals, DEX escrow `checked_sub`, BFT vertex payload cap (#200, #201).
  All change what a node accepts or selects, so they ship with the re-genesis.
- **Held for Eric (each changes consensus bytes):** DEX zero-quote dust (H-1; recommended:
  cancel the fully-consumed dust order and refuse placements under a minimum notional),
  governance quorum denominator (H-2), timestamp steering of the retarget (ADR-063 does not
  close it), retarget window off by one.
- **Reviewed, no change:** undo-journal optional section (local storage), snapshot chunk
  decode (allocates only what its input holds), empty-committee fallback (ADR-028 design),
  base fee saturating at `u64::MAX` (a ceiling by design), `HybridSigningKey` zeroize
  (both upstream key types are `ZeroizeOnDrop`, now pinned at compile time).
- **Dismissed with reason:** 7 CodeQL "hard-coded nonce" alerts, all in test files.

## 2026-10-27 — Audit decisions (Eric, all four "recommended")

- **H-1, DEX dust:** the matcher cancels and refunds the fully-consumed order on a zero-quote
  cross and keeps matching, and placement refuses orders below a minimum notional.
  Needs spec, conformance vectors, an ADR; the ignored test
  `a_resting_dust_order_does_not_freeze_the_honest_orders_behind_it` becomes the gate.
- **H-2, governance:** `eligible` includes total work credit, so turnout cannot exceed 100%
  and the denominator matches the vote weights.
- **ADR-063 accepted:** header version byte and median-time timestamp rule, plus a clamp on
  the retarget span input. Header 144 to 145 bytes; miner kernel, `NONCE_RANGE`, every block id,
  spec and vectors change. Free before genesis.
- **ADR-062 accepted:** sparse accounts tree and per-validator/per-delegation staking, built
  behind measurement; only what the numbers justify ships.
- **Not decided:** `halving_epochs` (needs a measured block rate on a network; the localnet
  benchmark is not one). The ticker is already MAYA (2026-10-18).

## 2026-10-27 — Implementation choices made while building the four decisions (Claude)

- **ADR-063 median rule is scoped to retargeting chains.** The existing clock-drift test
  showed it pushes DAG-BFT block times ahead of the clock when blocks come faster than
  about 6 a second, and a fixed-target chain has nothing to steer. The retarget measures
  between median times, which is what stops one extreme timestamp.
- **The header version byte is appended** (145 bytes) so `NONCE_RANGE` and the GPU kernels
  do not change.
- **H-2:** the work-credit clock now advances by whole half-lives; before, any record
  credited more often than every 5,760 blocks never decayed, which a chain-wide aggregate
  would have made permanent.
- **H-1:** the placement floor is "worth at least one quote unit" (what the arithmetic needs),
  not an economic minimum.
- **Proposed, not decided:** ADR-065, which cross-chain routes reach consensus (recommendation:
  swaps first, then Bitcoin inbound at activation height `u64::MAX`).


## 2026-10-27 — Eric's second round (Claude recorded)

- **ADR-062:** build now, one PR at a time. Per-validator staking split first, with a
  `bench-tps` before/after; the sparse tree only if block-apply time at 300k accounts justifies it.
- **ADR-065 accepted:** swaps first; Bitcoin inbound written but inactive (activation height `u64::MAX`).
- **Retarget off-by-one (audit 7):** fix before re-genesis, with spec, vectors and an ADR note.
- **Next gap:** `core::describe` in the GUI wallet and the explorer (kind column).

## 2026-10-27 — Third round: live-testnet freeze, release hold, halving (Eric; Claude recorded)

- **Live testnet is not touched** until its 7-day run finishes. No re-genesis, restart, deploy,
  or tag before then. Local work (tests, agents, benches, PRs) is unrestricted.
- **After the 7 days:** update the site/status/explorer with the result as proof, then
  attacknet, self pre-audit, mainnet. No further postponement of that order.
- **node-v0.1.0-testnet.5:** tag held for the re-genesis batch.
- **Explorer/status redeploy:** only after the 7-day finish.
- **`halving_epochs` delegated to Claude; decided: 35,064.** An epoch is 3,600 blocks
  (`DEFAULT_EPOCH_BLOCKS`). Cadence is set by genesis `round_interval_ms`, not emergent;
  `reports/18-economics.md` measured 0.96–1.03 blocks/s at 500 ms rounds on a devnet, so one epoch
  is about 1 hour and 35,064 epochs is about 4 years (4 × 365.25 × 24). It is the ADR-044 shape
  (about 4 years, computed from the measured rate). This is a devnet rate: if the mainnet
  `round_interval_ms` differs, recompute before the genesis freeze. It is Bitcoin's interval
  and is not claimed to be better than any chain's.
## 2026-10-28 — Supply, capacity and the governance deposit (Claude decided, Eric delegated)

- **Supply stays 21M coins at 18 decimals (2026-10-26 stands).** *[Superseded 2026-11-16, Eric 2026-11-14 MCQ: the headline is the 19,687,500 coins the four halving eras issue, as coded; the tail and the 7,000 bond coins are shown separately; 24 decimals since ADR-068.]* Capacity is already the
  maximum (U256, 2^256 base units, the EVM width), and 18 decimals is Ethereum's divisibility
  and 10^10 times Bitcoin's. A supply at the ceiling would make each coin worthless and
  overflow on the first reward. Eric's aim, "no numberwise disadvantage, and value", is met
  by maximum capacity plus a scarce supply.
- **Governance proposal deposit: default 10 coins, bounds 1 to 100,000 coins**
  (`crates/governance/src/params.rs`). The u64-era bounds were 1e-14 coin by default and
  1e-9 at most, so spam proposals were free and governance could not change that. It is
  refunded unless the proposal is withdrawn. Eric did not follow the question and asked
  Claude to pick; he can veto it before genesis.
- **ADR-062: both halves (Claude, under Eric's "enterprise grade" delegation).** Note that
  the existing sparse tree also rebuilds O(N) per block (docs/stateless.md, "Root cost"),
  so 062a means an *incremental* sparse root, pinned by an incremental == full-rebuild
  property test.
- **Mainnet moves to about 2026-11-20** (Eric accepted). Re-genesis soak runs in parallel
  with the self-run attacknet and pre-audit, not before them.

## 2026-10-28 — Forever mining, 24 decimals, fee burn, 512-bit math (Eric approved)

Eric: "OK I approve the chart keep the ecosystem upgradable and continute". Recorded in
[ADR-068](docs/adr/ADR-068-forever-mining.md):

1. A tail reward: the halvings stop at a floor paid every epoch to the last `u64` height.
   Genesis funds the reserve with the exact lifetime emission (no mint, ever); ceiling 2^192.
   **Built** (`state/mining.rs`, `genesis.rs`). Tail size: default proposal 0.5% of 21M a
   year, set in the genesis file at the re-genesis.
2. 24 decimals instead of 18. Not built yet.
3. Fee burn: already 100% of the base fee (ADR-029); more than the approved 50%, so unchanged.
4. 512-bit intermediates for amount × amount: audit of remaining sites not done yet.

"Upgradable": parameters change by protocol upgrade at an activation height, not by a live
governance knob over emission.

## 2026-10-28 — Validator keys are ML-DSA-87 (ADR-051 option 3, built)

Eric chose "Build it now" for the re-genesis. Separate type from transaction keys, `ml-dsa`
backend (invariant 4), context `maya2c validator v1`, spec CRY-4. Council keys stay
ML-DSA-65 for now. Every validator key must be regenerated for the re-genesis.

## 2026-10-28 — ADR-062a/b move to after mainnet (Claude, under Eric's delegation)

Mainnet deadline ~2026-11-28. Measured state-root cost is 0.88 ms at 1k accounts and 6.82 ms
at 10k (`reports/bench/state-root-scaling-23a158f4.md`); a launch chain sits at the low end.
The sparse accounts tree is already an activation-height switch (`sl:accounts-sparse`,
`state/stateless.rs`), so ADR-062a (incremental sparse root) lands later as a scheduled
upgrade without a re-genesis, and ADR-062b (per-validator staking records) the same way.
Building either now would put a new state format into the freeze week for no launch-time
gain. Revisit when accounts pass ~50k or block time is threatened (measure, then decide).

## 2026-10-28 — ADR-068 step 4: 512-bit product audit, clean

`rg` over node state/consensus, dex, fee-market, governance, ledger-math, staking and amount
for every multiplication of amounts. Every amount × amount product is a 512-bit
`mul_wide`/`mul_div`/`full_mul` (dex AMM invariant and share checks, governance tally,
ledger-math splits, fee-market step, invariant-guard anomaly) or a checked/saturating op that
refuses or clamps. Bare `*` on amounts appears only on constants (`10 * COIN`) and in
`staking/concentration.rs`, whose inputs are shifted into the `u64` range before `× BPS`
(fits `u128`). No change needed.

## 2026-10-29 — Pre-audit HIGH-4, LOW-7, INFO-8 (self-run audit by maya2c-9a)

- HIGH-4 fixed: every DAG-BFT vote signs `VOTE_DOMAIN ‖ H("maya2c dag-bft vote chain v1",
  chain_tag ‖ digest)`, so a validator key reused across chains cannot have votes or
  equivocations counted on another. Remote signers sign the bound digest unchanged. Wire
  change, pre-genesis.
- LOW-7: protocol floor unchanged (1 unit); mainnet genesis sets min_base_fee ≥ 10^6 (runbook).
- INFO-8: no protocol supply cap; genesis is the cap; with the tail, total > 21M by design
  (runbook). *[Superseded 2026-11-16, Eric 2026-11-14 MCQ: the headline is the 19,687,500 coins the four halving eras issue, as coded; the tail and the 7,000 bond coins are shown separately; 24 decimals since ADR-068.]*
## 2026-10-28 — No second public soak; the next public genesis is mainnet (Eric)

Relayed through session maya2c-9a, Eric's words: "we validate for 7 days and we continue, no
repeat". Overrides ADR-044's requirement that gate 3 run again on the mining build. The
frozen build is proven on local multi-validator networks (attacknet, rejoin drills, Hub
mining E2E, self pre-audit) from 11-04 to 11-08; mainnet 11-09 → 11-14, each step with its
own APPROVED. Risk stated once: the mining lane has never run on a public, multi-machine
network before mainnet.

## 2026-10-29 — Supply, miner pay, freeze rule (Eric delegated: "you decide the best for me")

- **No 512-bit amounts, no new minting.** Eric: "I don't wanna involve 512 bits". Supply
  stays ADR-068: about 21M coins over the halving phase, *[Superseded 2026-11-16, Eric 2026-11-14 MCQ: the headline is the 19,687,500 coins the four halving eras issue, as coded; the tail and the 7,000 bond coins are shown separately; 24 decimals since ADR-068.]* then a tail paid forever from the
  keyless genesis reserve sized to the last `u64` height. Nothing is ever minted, so the end
  of the halving phase changes nothing for miners: the tail simply continues. The 2^256
  base-unit total discussed on 10-29 is not built.
- **Tail: 1% of 21M a year** (about 210,000 coins a year, double the 0.5% default), set as
  `tail_epoch_reward` in `mining.json` at the re-genesis. Why: Eric wants miners paid well
  for good; 1% is close to Ethereum's issuance and the 100% base-fee burn (ADR-029) offsets it
  as use grows. Recompute the per-epoch figure from the mainnet epoch length before signing.
- **Freeze waits for the serious self-run findings**: S-012..S-015, S-027 (round 2) and
  S-033 (committee floor, ADR-070). Medium and low rows follow as patch releases.
- **Mainnet date: decided later, when the work is done** (Eric: "first finish the work").
- **Approvals (Eric, 2026-10-29/30, answering the decision list: "you are free to decide my
  brother", "whatever cause to dominate … should be done"):** the committee floor
  (ADR-070) is accepted; `APPROVED: re-genesis`, conditional on S-012..S-015, S-027 and S-033
  being merged and a full attacknet passing on that commit; `APPROVED: slip44`, submitted
  as satoshilabs/slips#2085 (coin type 2992, MAYA); listings (CoinGecko, CoinMarketCap) at the
  mainnet launch. Not covered: `sign genesis` and `mainnet` keep their own approvals.

## 2026-10-30 — Mainnet staking floors: 1,000 coins to validate, 1 coin to delegate (Claude, under Eric's delegation)

Eric, asked for these numbers: "finish everything … keep it up"; earlier in the same session
"you are free to decide my brother" and "do what's best financially and security wise".
Decided under that delegation:

- `min_self_bond` = 1,000 coins (10^27 base units). About 0.005% of the halving phase's
  issuance, so an early miner can reach it. Dust seats would otherwise fill the 100-seat
  committee (O(n²) DAG traffic) and churn for free.
- `min_delegation` = 1 coin (10^24 base units). One coin is cheap, but bounds how fast a
  sender can grow the staking record (the 8 MiB cap, S-006).

Set at the ceremony with `--min-self-bond` / `--min-delegation` (required on `maya-mainnet`)
and enforced by `cargo xtask genesis-check` (constants in `xtask/src/genesis_check.rs`).
Eric may change either before `APPROVED: sign genesis`; that is a one-line change.

## 2026-10-31 — Correction to "ADR-068 step 4: 512-bit product audit, clean" (2026-10-28)

That entry missed a vault limit bypass. `VAULT_FEE_HEADROOM = 4` exempted up to 4× the
required fee per transaction from the vault's instant limit, never charged to the window, so
a limit-0 vault could leak 3× required per transaction to the tip pool. Now only
`fees.required(size)` is exempt; anything above counts as outflow (PR #273, replay test
`a_fee_above_the_required_fee_counts_against_a_zero_limit`, fails on the old code).
Consensus-visible (invariant 32), rides the mainnet genesis. The saturating overflow first
suspected there is unreachable (`charge_fee` refuses first; balances ≤ supply).
Rule from here: an exemption from a limit is exactly the amount that justifies it.
## 2026-11-01 — 256-bit ceiling, DEX price scale 1e24, headroom uses (Eric)

Eric: "we always stay under 256 and use maximum we can use in 256 because going 512 is a
waste of computation power"; then, on the options: "use recommended maximum potential".

- **256-bit rule:** all new code computes in checked U256 (`checked_mul` -> typed error),
  no `mul_wide` / `Wide` / 512-bit `mul_div`. Bound: supply is about 2^104 (21M x 10^24),
  so amount x amount is at most 2^209.
- **Documented exception:** the existing 512-bit intermediates listed in
  `reports/sessions/2026-10-31-amount-product-audit.md`. Sites that also multiply by a
  fixed-point scale reach about 2^288, so a blind switch to checked U256 would refuse valid
  trades and rewards. A site is converted only with a written worst-case bound, a test at
  max inputs, and its Kani proof redone. Cost, for the record: a 512-bit mul_div is tens of
  ns against 1.062 ms per transfer (`reports/12-baseline.md`), under 0.01%.
- **DEX `PRICE_SCALE` 1e9 -> 1e24, before the freeze** (consensus format). It matches
  24-decimal coins: a price at a billionth of a coin per coin used to floor to 0 or 1, and
  now resolves exactly. Spot price is at most 2^104 x 2^80 = 2^184. Pinned by
  `crates/dex/tests/price_scale_tests.rs`. Safe only because mainnet starts from a fresh
  genesis: no order or vector at the old scale survives (a 1e9 price would mean 1e15x less).
- **Cumulative work cannot saturate on mainnet:** no change needed. A DAG-BFT genesis must
  set `difficulty_bits = pow_limit_bits = 0` (`genesis.rs:685`, `genesis_tests.rs:174`),
  so each block adds work 1 (`consensus_tests.rs:300`), and total work is at most the height
  (< 2^64).
- **After launch, at activation heights (BACKLOG P5):** reward-per-share accumulator,
  256-bit TWAP accumulators, packed state words.

## 2026-11-01 — Eric approves ADR-071 option B and the ADR-044 payout wording (economics)

Eric, 2026-11-01: "I approve everything, finish them".
- ADR-076 (was ADR-071; renumbered because 071 is taken): the base-fee ceiling is option B, 0.1 MAYA per standard transfer at the cap. That value is already in `crates/fee-market/src/limits.rs`, so no code change. Status: Accepted.
- ADR-044: the text now matches the code, with no consensus change.
  - A claim earns `budget / target_shares`, capped by what is left of the epoch's budget.
  - Unpaid budget stays in `MINING_RESERVE`, with nothing burned and no carry-over.
  - `R(e)` has a tail floor, and with a tail, genesis requires the reserve to hold `lifetime_emission()`.
  - The old text said pro-rata, remainder burned, and an empty epoch burned (`state/mining.rs` `share_reward`, `epoch_reward`; `genesis.rs` `bft.mining`).
- The GitHub `release` environment now requires Eric's approval and only deploys `node-v*` tags. The `pypi` environment requires his review.
## 2026-11-01 — No state transition may leave a funded pool without a usable price (consensus, freeze)

A funded pool whose spot price is 0, or overflows 256 bits, refunds every batch on its pair for
good, and its pair id cannot be reused. This was a pre-existing DoS flagged in the #279 security
review and agreed with maya2c-2b for the freeze. One predicate covers every path that writes a pool
(`has_usable_price` in `state/dex_exec.rs`):
- pool creation, deposit and withdrawal refuse the transaction before writing anything;
- a routed swap that would leave such a pool is a no-op, like an unpriceable leg, because an earlier
  transaction in the same block can move the pool and that must not void the block;
- batch settlement refunds the batch, as for an unpriceable pool, so a block is never voided.
Overflow cannot be reached: quote <= MAX_ASSET_SUPPLY = MAX / PRICE_SCALE. A partial withdrawal
that would end at price 0 is refused; a full withdrawal (pool empty) always passes.
Replays that fail on master and pass now: `a_pool_with_no_usable_price_cannot_be_created`,
`a_route_that_would_leave_no_usable_price_does_not_execute`. Regression guard (master already
refunds it): `a_batch_that_would_leave_no_usable_price_refunds_and_keeps_the_pool`.
Two more from the same security review, fixed in the freeze:
- Batch shrink: a zero clearing price, or a clearing that leaves spot 0, now drops the largest
  base seller (fallback: smallest, so the loop always shrinks). Dropping the smallest let one flood
  refund every honest co-trader. Replay: `an_honest_swap_still_clears_beside_a_flood_that_would_leave_no_price`.
- Withdrawal refuses only when both sides round to zero. Refusing when either side did stranded an
  LP's whole position in a pool with a dust side. Replay:
  `an_lp_whose_quote_share_rounds_to_zero_can_still_withdraw_its_base`.
Residual: "usable" means spot is not 0, not "every size clears". At spot near 1, small base sells
round to 0 and are refunded until a buyer moves the price. That is a refund, not a lock.

## 2026-11-02 — The proof ratchet (ADR-078 O3)

`cargo xtask proof-ratchet --check` runs in CI beside claims-check. It fails
when any of these falls below `proof-ratchet.txt`:
- Kani harnesses;
- spec conformance vectors;
- exploit replays;
- ACVP vector files;
- fuzz targets.

Baseline at creation: 83 harnesses, 164 vectors, 11 replays, 8 ACVP files,
16 fuzz targets. Removing a counter also fails, so the ratchet cannot be
bypassed by deleting it. A count that rises is raised with `--update`, and the
commit message says why.
## 2026-11-02 — Sentinel Recovery Network proposed (MIP-0001, ADR-077)

Eric asked for a protocol-level disaster-recovery upgrade. The design is written as
MIP-0001, with ADR-077 Proposed and seven open decisions (D1–D7). Departures from
the brief, each argued in MIP-0001 §0.1:
- no rewind of finalized blocks; recovery is a forward restitution of tainted
  balances;
- AI is advisory only (invariant 20);
- quarantine is per cell, never chain-wide, except a bounded T5;
- penalties fall only on posted bonds;
- recovery stays off until the committee passes an independence floor, so it is
  off at launch (ADR-047).

The prior-art search (2026-11-02) found Sui, Berachain, Flow and BNB Chain
interventions, Cosmos `x/circuit` and Forta, so no "first" claim is made.
Nothing enters the launch binary.

Eric approved D1–D7 as recommended the same day ("I approve all you recomended"). ADR-077 is Accepted, and ADR-050 is amended for tainted balances once SRN activates. D5's exact fee share comes back to him as a number with R6.

## 2026-11-02 — Omega design proposed (MIP-0002, ADR-078)

Eric asked what Maya2C must become so that no future protocol can surpass it.
The answer, written honestly: no design can guarantee that, so MIP-0002 designs
three things instead:
- advantages a fork cannot take: track record, immunity corpus, identity graph,
  proofs and decentralization, each with a published indicator;
- an absorption pipeline for outside breakthroughs;
- provable self-steering: bounded integer controllers, evolution searched off
  chain and selected on chain.

Decisions O1–O4 are proposed:
- O1: three-speed evolution with pre-shipped, vote-activated code;
- O2: bounded controllers;
- O3: a test ratchet, which can start now;
- O4: moat indicators.

Prior art: Tezos and Polkadot already upgrade without hard forks. Maya2C's
difference is that a vote selects only the activation height, never code from
state (invariant 13).

Eric approved O1–O4 the same day. ADR-078 is Accepted. O3 (test ratchet) starts now in CI.
## 2026-11-01 — Reviewed guards are marked in code, not P0 rows

The three P0 rows judged guards on 2026-09-28 now carry a `reviewed-guard:` comment, and
`cargo xtask gap-register` lists such rows as `guard` rather than P0/P1. They stay in the
register so the call can still be disputed. Kani proof modules (`#[cfg(kani)] mod x;`) are
treated as test code: `fee-market/src/proofs.rs:129` was counted as a P0 though it is in no
binary. Register now: 0 P0, 0 P1, 31 P2, 3 reviewed guards.

## 2026-11-02 — No second long soak; test networks run one day at most

Eric: the 7-day validator soak is finished, and there will be no second 7-day
run. A proving run or temporary testnet lasts at most 24 hours. `cargo xtask
proving` now defaults to 24 h, the post-proving checklist says 24 h, and the
re-freeze's proving restart uses it.
## 2026-11-02 — B49: a stale node rejoins only through a trusted block (node policy, not consensus)

A node whose committed state is older than the unbonding window cannot tell the real chain from
a long-range forgery by its own state: the validators that signed it have unbonded and their
old keys sign at no cost. Rule (`bins/maya2c-node/src/rejoin.rs`, pure `stale()`):

- **Epoch trigger:** the newest checkpoint any answering source claims is more than
  `unbonding_epochs` epochs past the committed staking epoch. Exactly `unbonding_epochs` behind is
  not stale; one more is.
- **Time trigger:** the tip header is older than `unbonding_epochs × epoch_blocks ×
  TARGET_BLOCK_TIME` seconds. This is derivable, but `TARGET_BLOCK_TIME` (15 s) is the intended
  rate, not a measured one, so the window is an estimate. The epoch trigger does not depend on it.
- A stale node with no `--trusted-block` is refused at start-up, with the way out in the message.
- With a trusted block, the gap below it is linked to that block by hash alone (no committee),
  and the committee rule resumes above it. A batch that reaches the trusted height must carry its
  id there, or nothing is imported. Certificate replay (ADR-052) is skipped when a trusted block
  is in use, because it verifies against the committee and would ignore the anchor.

CLI: `--trusted-block` is accepted with `--bootstrap-from` or `--catch-up-from`, and refused
otherwise (an anchor nothing uses is refused, not ignored). Staleness is decided once at start-up;
the live follower (`follow_loop`) is unchanged and still uses the genesis committee as fallback
for chains without staking. Not done: chains without staking state (the guard does not apply),
and a node that goes stale while running.

### B49 review round (2026-11-02, after the security and Rust reviews of #324)

Superseded in part by the entry above:

- Start-up runs with or without `--catch-up-from` (`rejoin::anchor`). A stale node with no
  trusted block is refused in both cases.
- A trusted block at or below the local tip must be this node's block at that height
  (`check_against_tip`). A trusted block above the tip is checked as it is reached.
- A stale node must reach its trusted block in the start-up (`require_reached`). If no source
  answered, or none served a chain through it, the node does not start.
- A trusted block must be RECENT: for an `Epochs` stale reason, its epoch must be within the
  window of the claimed head (`is_recent`). Epoch of a height is `height / epoch_blocks`, since
  the staking epoch ends at each multiple of `epoch_blocks` (`staking_exec.rs`).
- The claimed epoch is UNVERIFIED. It only tightens the guard: a hostile source can force the
  requirement (fail-safe). It does not lift it: see the trust model in the re-review entry below.
- A tip later than the clock, beyond `CLOCK_SKEW_SECS` (120 s), is stale (`FutureTip`). A clock
  that cannot be read is `u64::MAX`, which makes every tip stale (`clock_now`).
- A running follower re-checks staleness each round (time trigger). Stale with nothing pending
  stops the follower, and the node exits. Certificate replay (rescue) is requested only once the
  trusted block is held.
- Linking across more than `MAX_LINKED_GAP` = 50,000 blocks is refused before any request
  (`catchup::check_gap`). The cap counts blocks; bytes are not bounded (shortcut recorded there).
- Probes use `RpcBootstrapSource::probe_checkpoint`: one attempt, `PROBE_TIMEOUT` = 10 s, no retry
  deadline. Probe failures are printed, not dropped.
- Windows (`7 x epoch_blocks x 15 s`): testnet 105 h, Oracle VM and devnet 35 min, attacknet
  1.75 h. Mainnet `epoch_blocks` is not in this repo. Operational consequences are in
  `docs/runbooks/hybrid-testnet-hardening.md` section 4.

### B49 re-review (2026-11-02): confirmation required, byte cap, recency on the max claim

Supersedes the earlier round where they conflict.

- **Staleness needs a confirming head.** A node's own clock or tip never refuses alone. The
  time trigger fires only when the tip is older than the window AND a source reports a head
  more than `unbonding_epochs x epoch_blocks` blocks past the tip. A time-old tip with no newer
  head starts with a warning. This is what lets a genesis-only node start, and lets a halted
  network keep running. A future tip is a warning (`future_ahead`), not a refusal.
- **Recency is checked against the maximum head any source reports, for every stale reason.**
  A lower claim from another source cannot loosen it while the maximum is kept.
- **Followers** probe only while the tip is old, at most every `FOLLOW_PROBE_EVERY` = 30
  rounds. A follower exits only when a source confirms a head beyond the window and no trusted
  block is pending.
- **Byte cap:** `MAX_LINKED_GAP_BYTES` = 512 MiB, checked as each response arrives
  (`catchup::fetch_linked`). Why 512 MiB: the block cap alone does not bound memory, since a
  block is up to 5 MiB (ADR-072). The cap binds first for blocks above about 10.7 KiB.
- **Real maximum offline span through a trusted block:** the 50,000-block cap (8.7 days at 15 s)
  plus one window of the head (4.4 days on 3,600-block epochs), about 13 days. The byte cap
  shortens this for large blocks. Beyond it, `--bootstrap-from`.
- **Known residual (LOW):** a P2P-only node (no `--catch-up-from`) that goes stale while running
  has no follower to stop. It keeps running. A restart applies the start-up rule.
- **Unverified claims:** a source can force the requirement (fail-safe), and it can make
  recency stricter.

### B49 round 4 (2026-11-02): eclipse bypass closed, trust model stated

The re-review found that a node whose every source returns no head, or a low one, gets no
confirmation. It then follows a forged chain on its old committee, ignoring its own clock
evidence. A node alone cannot tell "halted network or old genesis" from "eclipsed", so the
operator says which.

- **A time-old tip with no confirming head is refused at start-up, at any height**, unless the
  operator passes `--allow-unconfirmed-start`. The refusal names both honest causes and the
  attack, and points to `--trusted-block` or `--bootstrap-from`. A fresh genesis inside the
  window starts with no flag (`decide`, `rejoin.rs`).
- **A time-old tip with a `--trusted-block` and no claim** takes the hash-linked path
  (`Anchor::Stale`). The operator opted in with a block, so the hash-linked path applies even
  though no head confirms staleness. Recency cannot be checked without a head, so the operator's
  block choice carries that risk.
- **Running followers are unchanged.** A follower with no confirming head keeps following and
  warns, and it does not exit. The residual: an eclipsed follower keeps following a forged
  chain while its sources withhold the head. Only a confirmed head beyond the window stops it.
- **Recency is tightened by one epoch.** A trusted block must be at least one epoch inside the
  window of the head (`is_recent`, strict `<`). A block exactly at the edge would put the
  follower straight back into stale mid-catch-up.

**Trust model.** A running follower assumes at least one honest source. A stale restart is
safe with `--trusted-block`, or with the operator's explicit `--allow-unconfirmed-start`. An
operator should list two or more `https` sources, so that one hostile source cannot be the only
voice. Heads are unverified, and the maximum over sources decides the requirement and the
recency. A hostile source can force the requirement (fail-safe). It cannot lift it while any
honest source answers with the head.

**Byte cap, true bound.** The cap is 512 MiB, but responses already in flight when the total
passes it are still held. With `PREFETCH_WORKERS` = 4 and 32 blocks of 5 MiB per request, the
peak is about 512 MiB + 640 MiB, roughly 1.2 GiB. This is recorded in `catchup.rs` at
`MAX_LINKED_GAP_BYTES`.

### B49 final review (2026-11-02): measured block interval, own-block trusted refused

- **Block interval is measured, capped at the target.** `block_interval_secs` (`rejoin.rs`)
  takes `(tip.timestamp − timestamp of the block epoch_blocks back) / epoch_blocks` from the
  node's own headers, and uses `min(observed, TARGET_BLOCK_TIME)`, at least 1 s. With no
  history, or a non-positive delta, it uses the target. This chain's DAG-BFT blocks have been
  measured well under 15 s (`reports/bench/2026-10-25-tps-series.md`, about 2.6 s per block on a
  shared machine). A shorter window is the strict direction: it can only make the time trigger
  fire sooner. An honest late restart has `--trusted-block` or `--allow-unconfirmed-start`.
- **A `--trusted-block` at or below this node's own tip does not lift the unconfirmed
  refusal.** It would give a catch-up of nothing. Only a block above the tip gives the
  hash-linked path.
- The earlier sentence claiming that a fully hostile source set leaves only the time trigger
  is removed from `DECISIONS.md`. The trust model above replaces it.

## 2026-11-07 — Upgrade schedule on chain, under compiled leads; council fast path limited to sunsets (ADR-082)

- **Decision:** the schedule is committed state (`up:`), written by governance `ScheduleUpgrade` (30-day lead, `MIN_UPGRADE_LEAD_BLOCKS`) or by the security council `SunsetSuites` (no emergency window; lead `COUNCIL_SUNSET_LEAD_BLOCKS`).
- **Council lead, derived:** `COUNCIL_SUNSET_LEAD_BLOCKS = vault::MAX_DELAY_BLOCKS + agility::MIN_MIGRATION_WINDOW` = 1,000,000 + 172,800 = **1,172,800 blocks, about 13.6 days at one block per second.** The earlier draft's 7 days was the migration window alone, without the vault delay. The reviewer's "about 18.6 days" assumes a 7-day migration window (604,800 blocks). The code's `MIN_MIGRATION_WINDOW` is 172,800 (2 days), so the derived figure is 13.6 days. The constant is derived, not hard-coded. Changing the migration window to 604,800 would make the lead 18.6 days. Decide that before mainnet.
- **Why the bound:** a vault delay is at most `MAX_DELAY_BLOCKS` blocks. A council sunset announced just before a withdrawal could freeze funds past the sunset height unless the lead covers the delay. The migration window is the time users have to move funds to another suite.
- **No exit-only mode:** an exit a broken suite can sign would let an attacker exit a victim's funds too. Users migrate by transferring to an address under another suite.
- **Recorded in:** ADR-082, invariant 34, `crates/node/tests/council_pause_tests.rs`, `crates/node/tests/sunset_tests.rs`.

## 2026-11-02 — B51: join scripts seal the validator key with a generated passphrase file (ADR-049, operator tooling)

- Production builds refuse plaintext keys, so the installers (`join-validator.ps1`, `join-testnet.sh`) now make a 32-byte CSPRNG passphrase in `validator.pass` next to the key (`C:\ProgramData\Maya2C\`, `/var/lib/maya2c/`, `/Library/Application Support/Maya2C/`), pass it to keygen by file, and export `MAYA2C_VALIDATOR_PASSPHRASE_FILE` on every start. The passphrase never goes on a command line or into a log. Reinstall keeps it; Windows `-Everything` removes it with the key. Losing it loses the key, so the operator is told to back it up. Windows ACL: grant SYSTEM and Administrators first, then `/inheritance:r`. Unix: `umask 077`, mode 600.
- The Mac installer's run script now quotes its paths. macOS keeps data under `/Library/Application Support`, which word-split in the old `exec` line.
- `infra/testnet-vm/install.sh` (the single-VM testnet) follows the same rule: one `validator.pass` per `v<i>` directory, passed to keygen by file and set by `Environment=` in `maya2c-validator@.service`. A reinstall that finds a key without its passphrase file stops; it never makes a new passphrase for an existing key.

## 2026-11-02 — Owner approvals "approve all, recommended options": fee ceiling, supply wording, treasury, council, docs

Owner approved the recommended option for each item below. Docs only; no consensus code changed.

- **A4, fee ceiling (ADR-076, was ADR-071):** option B kept, 0.1 MAYA per standard transfer at the cap. ADR-076 status is Accepted (2026-11-01); no change.
- **A5 and B43, supply wording (ADR-044):** the text now matches the code.
  - One coin is 10^24 base units (`maya_amount::COIN_DECIMALS = 24`, `ONE_COIN = 10^24`, `crates/amount/src/lib.rs`).
  - `fee_market::supply::MAX_SUPPLY` = `Amount::MAX` = 2^256 − 1 base units (`crates/fee-market/src/supply.rs`).
  - 21,000,000 coins is the halving-phase emission budget (ADR-068), not the cap. *[Superseded 2026-11-16, Eric 2026-11-14 MCQ: the headline is the 19,687,500 coins the four halving eras issue, as coded; the tail and the 7,000 bond coins are shown separately; 24 decimals since ADR-068.]* The old "21M × 10^8" wording is removed from ADR-044 and `docs/fee-market.md`.
  - The reserve is an explicit allocation to the keyless `MINING_RESERVE`, not `MAX_SUPPLY − Σ allocations`. With a tail, genesis refuses a reserve below `lifetime_emission()` (`crates/node/src/genesis.rs`).
- **B-16, fee treasury (code check):** the 20% share is burned today, as decided.
  - The node sets `treasury_bps: 0` (`crates/node/src/genesis.rs`). The base fee moves wholly to `FEE_SINK` (`crates/node/src/state/fee_exec.rs`, `settle_fees`).
  - The 80/20 split in `crates/fee-market/src/execute.rs` has no caller outside the crate.
  - `docs/fee-market.md` and `docs/site/authored/transactions.mdx` now say so.
  - Still differs: `spec/04-fees.md` FEE-1 defines the treasury split as live, and the crate default `FeeConfig::TESTING` uses 2000 bps. Not changed here (consensus spec). Open item. *[Closed 2026-11-16, Eric 2026-11-14 MCQ: FEE-1 now states the 100% base-fee burn (spec 4 v0.1.1), and the econ model runs at `treasury_bps = 0`.]*
- **B30, council (ADR-050):** `max_pause_blocks` = 6 hours of blocks, the value ADR-050 already records (2026-10-19 settings). The block count is computed at the ceremony. The offline council key is held in owner-held offline hardware; the location is not published. ADR-050 now carries this decision.
- **B45:** the `release/*` ruleset is applied by the lead session, not by this PR.
- **C15, doc drift:**
  - `docs/architecture-vision.md` line 96 already reads SHIPPED / ADR-027 on master. No edit.
  - Validator votes are ML-DSA-87 in code (`crates/node/src/consensus/bft/auth.rs`, ADR-051 option 3). `docs/site/authored/quickstart.mdx` said ML-DSA-65; fixed. ADR-027 carries a supersede note. `consensus.mdx` and `validators.mdx` already said 87.
  - The macOS Hub installer claim in `docs/site/authored/mining.md` is corrected: no Hub installer is published for any platform.
  - Organization JSON-LD `sameAs` (`docs/site/astro.config.mjs`) keeps only the GitHub project repository. LinkedIn, Facebook, Instagram and the personal subdomain are removed.
- **B27, bootnode:** the mainnet bootnode peer id is `MAINNET_BOOTNODE_PEER_ID=SET-AT-GENESIS` in `deploy/genesis/mainnet/bootnode.env`. The mainnet join script does not exist yet. The testnet guides and `scripts/join-testnet.sh` keep the testnet id. No validators-guide generator exists, so the guide is hand-maintained and was not regenerated.

## 2026-11-01 — B50: work claims refused at launch until bound to a verified share (ADR-044, ADR-079)

- **Decision:** governance voting is stake-only at launch. `TxKind::ClaimWork` is refused at every height below `WORK_CLAIM_ACTIVATION_HEIGHT` (`u64::MAX`), before any credit or fee, with `NodeError::WorkClaimInactive`. Code: `crates/node/src/state/context.rs`, `crates/node/src/state/settlement.rs`, shipped in #317 (`f7f0958b`).
- **Why:** `claim_work` credited genesis-target work to any named beneficiary with no proof the sender mined, so one fee per block bought voting weight.
- **Lifting it:** only through a written activation height, bound to a claim that is backed by a verified share in the same block and credited to that share's miner (ADR-044). ADR-079 R2 is the first place that may do this; it needs its own consensus ADR.
- **Recorded in:** ADR-044 ("Work credit at launch"), `docs/audit/KNOWN_ISSUES.md` #2 and #18, tests in `crates/node/tests/governance_tests.rs`.

## 2026-11-07 — Zero premine with minimum disclosed validator bonds; tail rate accepted (Eric)

Eric decided both open questions from the genesis audit (PR #374). Decision only; the code and docs follow in the same branch.

- **Zero premine (ADR-044):** mainnet launches with `--supply 0`. The ceremony refuses any other supply on `maya-mainnet` and writes no root key or treasury. Genesis money is exactly the mining reserve plus the validator bonds; `genesis-check` refuses any other allocation.
- **Tiny bonds:** every mainnet validator bonds exactly `--min-self-bond` (1,000 coins). The ceremony and `genesis-check` both refuse any other bond, so stake concentration is the seat count, which the 20% operator cap already bounds.
- **Tail rate accepted:** about 1.07% a year at one block a second (210,000 coins a year on 19.69M). The rate follows block speed. It can be retuned later through an activation-height protocol upgrade.
- **Upgrade authority (Eric 2026-11-07):** protocol upgrades are scheduled on chain. A governance `ScheduleUpgrade` needs at least about 30 days of lead time. The 2-of-3 security council has a fast path for suite sunsets only, with at least about 7 days of lead time. Neither path can move or mint funds. Implemented in PR #376 (ADR-082).
- **Address format (Eric 2026-11-07):** Bech32m text form, with human-readable prefix `maya` on mainnet, `tmaya` on testnet and `dmaya` on devnet. Implemented in PR #375 (ADR-081).
- **Recorded in:** `deploy/genesis/mainnet/README.md`, `docs/runbooks/regenesis-checklist.md`, ADR-068, `bins/genesis-ceremony`, `xtask/src/genesis_check.rs`.

## 2026-11-08 — Mainnet min-self-bond is 1,000 coins; genesis carries 7,000 coins of bonds (Claude, under Eric's pre-approval)

A draft said 100 coins per bond. The tool does not allow that: `genesis-check` floors `min_self_bond` at 1,000 coins (decided 2026-10-30). Claude took the floor as the value, so the bond the ceremony writes is the one the tool enforces.

- Each of the 7 genesis validators bonds exactly `min_self_bond` = 1,000 coins (10^27 base units). Genesis holds 7,000 coins of bonds, about 0.036% of the 19.69M era issuance.
- The bonds are disclosed because they are minted value at genesis; they appear in the mainnet README and the checklist, not hidden.
- Mainnet launches with an empty protocol-upgrade schedule (ADR-082): `genesis-check` refuses any entry for `maya-mainnet`. Other chains may carry launch entries.
- Recorded in `deploy/genesis/mainnet/README.md`, `docs/runbooks/regenesis-checklist.md`, `docs/GENESIS.md` and STATE.md.

## 2026-11-08 — Reward split math stays within 256 bits; seven launch decisions confirmed (Eric)

Eric decided the open items from the pre-freeze review. These are decisions only, except the first, which lands with this entry.

- **`bps_of` in checked 256-bit math:** `bps_of` in `crates/node/src/state/mining.rs` now multiplies with `checked_mul` and divides with `checked_div`, instead of using `mul_div`'s 512-bit product (the 2026-11-01 rule: convert only with a written bound). The bound: every schedule value is at most `LIFETIME_EMISSION_CEILING` = 2^192, which `validate` enforces on every record, and bps is clamped to 10,000 < 2^14, so the product is below 2^206. A `const` assertion pins it. It is the same floor division of the same exact product, so the output does not change. A deterministic sweep against the old `mul_div` and the mainnet emission pins check this, since any difference would be a hard fork. Out of bound it returns `value`, the old fallback. The other 512-bit sites stay as they are until Eric decides each one.
- **License:** stays Apache-2.0, with the brand protected by `BRAND.md` (PR #387).
- **Validator bond:** 1,000 coins for each of the 7 genesis validators (entry above).
- **Council sunset lead:** 13.6 days (1,172,800 blocks, derived in ADR-082). The 2-day migration window stays.
- **Admin bypass:** removed from the `master` ruleset, so admins also merge only through the required checks.
- **Founder mining:** the founder mines from block 1, and this is disclosed publicly.
- **Watchdog alerts:** sent through ntfy.sh.
- **BitLocker:** turned on for the validator PC before genesis.

## 2026-11-14 — Council hardening before genesis: no Governance pause, a renewal cooldown, key rotation (Eric 2026-11-14)

Eric chose the recommended answers to founder-audit Q7 and Q8 ("I approve everything, proceed", Eric 2026-11-14). ADR-082 amendment 2026-11-14, invariant 34, spec CNL-1 to CNL-4. Status: decided; the implementation is PR #544, pending merge and green CI.

- **Q7 A, Governance is not council-pausable:** a council pause or resume that names Governance is refused. The council could otherwise freeze the stake vote that answers it. Trade-off: the council can no longer slow an attack on governance itself, and since `Module::Governance` also covers mining shares and work claims (ADR-044), it can no longer halt those payouts either. Deviation from A's wording ("remove Governance from MODULES"): the tag stays in `MODULES`, because tags are permanent, and the action is refused instead.
- **Cooldown:** `COUNCIL_RENEWAL_COOLDOWN_BLOCKS` = 86,400 (one day at one block per second). A council pause of a module is refused while the last one holds and for a day after it ends or is resumed. The end is kept per module in the council record (`last_pause_end`), so an automatic breaker trip, which overwrites the breaker record, does not reset it (PR #544 review). A council sunset is refused within a day of the council's last sunset. The value is this session's choice under Eric's approval of the recommended option. The audit named no figure. Residual, from the audit: nothing caps how many modules are paused at once.
- **Q8 A, rotation:** `CouncilKind::Rotate`. A quorum of the council in force names a new set: 1 to 32 distinct keys under a strict majority, and every new member signs an acceptance, so a key nobody holds cannot enter the set. The set takes over no sooner than `COUNCIL_ROTATION_LEAD_BLOCKS` = 86,400 after the block that carries it. It is public in the council record (`pending`) for that whole time. A later rotation replaces a pending one. Limit: rotation needs a live quorum; with more keys lost than the threshold allows, re-genesis is still the only path.
- **Genesis change:** genesis now applies the same member rule, so a genesis council of more than 32 members is refused. Before, genesis had no upper bound; such a council could never have acted.
- **Pause ceiling (PR #544 review, 2026-11-17):** a genesis `max_pause_blocks` above `MAX_COUNCIL_PAUSE_BLOCKS` = 86,400 (equal to the cooldown) is refused, as is 0. Without it a genesis could set a pause of years and the cooldown would bound nothing. The B30 choice, six hours of blocks, is inside it.
- **Not built:** the brief asked for the rotation to be "logged as an event". The chain has no event log outside the VM; the transaction and the `pending` record are the log.
- **Not decided here:** Q9, who holds the keys. It stays one operator's three devices (2026-10-18 entry) until Eric decides otherwise.
- **Handover advances the nonce (PR #544 review, 2026-11-21, Eric 2026-11-14):** an approval signs the nonce, not the member set, so before this a key kept across a rotation could have its approval, given under the old quorum, counted toward the new one. `CouncilRecord::at` now advances the nonce by one when a rotation takes over. The alternative, signing a hash of the member set in force, changes the signed bytes; one line in a pure function was the smaller change with the same effect. The cooldowns carry across the handover, now pinned by a test. A council resume of an automatic trip also starts the council cooldown on that module; kept and written in ADR-082 as a residual.
## 2026-11-14 — The neural fee model is deleted from the launch binary

Eric 2026-11-14 ("I approve everything, proceed"; founder-brief audit Q19,
recommended option A). The neural base-fee gain sat in `maya-fee-market`
and the node's `neural_gas` module at activation `u64::MAX`: dormant, but
compiled into the binary genesis freezes, against the rule that models never
sit on consensus. The model and its rule moved to `crates/neural-fee`
(RESEARCH, linked only by `neural-gas-trainer`); `FeeConfig` lost its neural
switch; the node's feature extractor, its join test and its bench are
deleted. Fee behaviour is unchanged on every network: no network set the
height and `FeeConfig` has no serialized form. Checked 2026-11-14 by `cargo
tree -i maya-neural-fee` on `custom-l1-node` and `maya2c-node` (not in either
graph), a `Cargo.lock` walk in `fee_market_tests`, and the fee-market,
neural-fee, trainer and node fee test runs recorded in the ADR-029 amendment.
The trainer's weight-drift check fails, as it already did at `3735252d`.

## 2026-11-22 — S-033 at mainnet bonds: option A + B (decision brief 1) — PENDING ERIC

Not decided. Claude's recommendation, built as a `[needs-eric]` draft PR stacked on #524, from
owner decision brief 1 of the independent AI audit (2026-11-19; an AI review, not an external audit).

- **Option A** (count the floor before the slash; a floor-kept validator stays eligible) is
  #524's rule, unchanged (entry of 2026-11-21 below).
- **Option B, proposed:** every mainnet seat bonds exactly the genesis bond, `min_self_bond`
  plus 10.6% (1,106 coins at the 1,000-coin floor), enforced by `genesis-check` and the ceremony.
  It replaces "every bond exactly the minimum" of 2026-11-07 / 2026-11-08 if Eric takes it.
  Why: at bonds of exactly the minimum a floor-kept validator can never be slashed, and a
  jailed one cannot return without a top-up; with the margin both hold for ten outages.
- **What changes at genesis:** 7,742 coins of bonds instead of 7,000; the node is unchanged.
- **Not taken:** C (no slash while the floor binds) and D (accept); reasons in ADR-070
  amendment 2026-11-22.
- **Eric decides:** A + B, A alone (#524 as is), or another option.

## 2026-11-21 — S-033 committee floor holds at mainnet bonds (Eric 2026-11-14)

Founder-brief audit question 17. Eric 2026-11-14 ("I approve everything, proceed") took the
recommended option A, "merge the fix before the freeze".

- **The merged floor did not protect a mainnet validator.** #246 (46a97a61, in the freeze
  bb4e0940) counted the floor after the 1% downtime slash, and mainnet bonds equal
  `min_self_bond` exactly (`xtask genesis-check`), so a down validator fell below the
  minimum and was jailed whatever the floor said. The brief assumed a fix only needed
  merging; it needed a rule change (Standing Order 9: reported in PR #524).
- **Chosen: option (a)** of the ADR-070 amendment 2026-11-21. The floor is counted before
  the slash; a kept validator is unpaid but slashed only if it stays at or above
  `min_self_bond`. Not (b), which removes the downtime slash for everyone at mainnet bonds,
  and not (c), which reverses the tiny-bond decision. Consensus-visible, so re-freeze work:
  PR #524 is retitled `[re-freeze]` and waits for Eric's merge with the re-genesis.
- **Economics note for Eric:** at mainnet bonds a kept validator loses its epoch reward but
  no bond. This is the one place the change touches economics; say so if you want (b) or
  (c) instead.
- **Beyond any fix**: more than f seated validators gone for good leaves no quorum to reseat
  anyone (the 2f + 1 rule); recovery is the social re-genesis (`docs/audit/KNOWN_ISSUES.md` #18).
- **Not decided here**: `cargo xtask go-no-go` counts every self-run `high` row that is not
  `closed` as a NO-GO; S-033 is `fixed`, not `closed`.
## 2026-11-14 — Notary (ADR-094) stays a tool and moves to working on its devnet test (Claude, under Eric's approval, Eric 2026-11-14)

- **Tool only:** the notary is `maya2c notary` on the existing `AnchorAttestation` payload (tag 34, schema `notary-v1`). No consensus, wire or state change; no node RPC added.
- **Status:** `features.toml` `notary` moves from stub to working on `bins/maya2c-cli/tests/notary_devnet_tests.rs` (a stamp lands in a block on a dev node, the receipt matches that block, one changed byte is refused). Class stays RESEARCH: no consensus path calls it.
- **Verify hardening:** verify checks each served block's body against the `tx_root` in the header its id hashes (not the separate JSON field), checks the id is the one named, refuses a non-contiguous batch, and checks the stamp's signature.
- **Shortcuts kept:** finality is the node's checkpoint with committee signatures unverified, and the chain tag comes from the node's `get_chain_info`.
- **Open anomaly:** one early manual run never saw the stamp land; it was not reproduced and its cause is unknown.

- **Tool only:** the notary is `maya2c notary` on the existing `AnchorAttestation` payload (tag 34, schema `notary-v1`). No consensus, wire or state change; no node RPC added.
- **Status:** `features.toml` `notary` moves from stub to working on `bins/maya2c-cli/tests/notary_devnet_tests.rs` (a stamp lands in a block on a dev node, the receipt matches that block, one changed byte is refused). Class stays RESEARCH: no consensus path calls it.
- **Verify hardening:** verify checks each served block's body against the `tx_root` in the header its id hashes (not the separate JSON field), checks the id is the one named, refuses a non-contiguous batch, and checks the stamp's signature.
- **Shortcuts kept:** finality is the node's checkpoint with committee signatures unverified, and the chain tag comes from the node's `get_chain_info`.
- **Open anomaly:** one early manual run never saw the stamp land; it was not reproduced and its cause is unknown.
## 2026-11-14 — Contract gas is capped per transaction and per block, and native work is priced (Eric 2026-11-14 MCQ, ADR-142)

Eric chose "cap + price now" in the 2026-11-14 MCQ. A call's `gas_limit` was sender-chosen up to `u64::MAX`, nothing bounded a block's contract work, host functions and the per-call module compile were unmetered, and every call loaded the contract's whole storage. Now:

- **Caps:** `vm.max_tx_gas` (tag 17) and `vm.max_block_gas` (tag 18), 250 M each, governed within compiled bounds `[239,979,600, 500,000,000]`; a call is capped at the smaller, a block's calls and deployments share the block cap. Refused at admission and in block validation; the builder leaves what does not fit for a later block.
- **Prices** (`crates/vm/src/gas.rs`), at about 1 ns of measured worst-case native work per unit: 50,000 per call, 450 per module byte on a contract's first call in a block (the compile) and 1 after, 600 per host call, storage reads 5,000 + 1/byte, writes 5,000 + 240/byte (priced by persistence: ≤ ~1 MiB of new state per block), events 600 + 1/byte. No refunds.
- **Lazy storage:** the host reads committed storage one key at a time; randomized equivalence against the old whole-storage load is a test.
- **Not decided here, left open:** a fee per gas (economics, Eric's), charging failed plaintext calls (changes STF-8), and an incremental contracts root (the root still folds all contract state each block).

## 2026-11-14 — ADR-142 amended after the PR #549 review (Eric 2026-11-14 MCQ, same "cap + price now" decision)

The review found work running below its price or outside the block's budget; measured on this PC, the worst was a module of 200 functions with 50,000 locals each that compiled in 678 ms (release) against a price of 0.78 M gas, and 40,000 `if`s in one function that compiled in 10.4 s. Implementing "cap + price now" fully therefore needed:

- **Compile priced by shape** (`crates/vm/src/shape.rs`): a linear pre-pass before any compile; at most 1,024 locals and 2,000 control operators per function and 10,000 functions; cold price adds 125,000 per function, 170 per local, 30,000 per control operator, 8,000 per call; a module whose cold price exceeds 235,979,600 is refused at deployment, so the governance floor still keeps every deployed module callable.
- **Linear memory priced** at 6,000 a page held (GAS-7), and a call may hold only what its gas pays for.
- **Refused host calls trap**, and event topics are capped at 256 bytes.
- **Failed work stays counted:** a failed call spends the gas it used (not its limit) and a failed deployment its price, counted before the compile, in block selection and in revealed sealed actions.
- **Revealed sealed actions get their own budget**, a quarter of `vm.max_block_gas`, so a plaintext call cannot void a due sealed call.
- **Warm and cold storage reads:** the block's first read of a committed key pays 95,000 more (measured 73–113 µs from disk with direct I/O). A bloom filter was measured and left out (it made hits slower with a small cache).
- **No per-call copies** of the block's staged contract storage; contract trials no longer clone the overlay.
- **Ceiling 300 M** for both caps (was 500 M).

Raised with Eric, not decided here: one gas-filling call per block crowds out other contract calls for a byte fee (fee per gas or a per-sender share is economics); failing calls are free and stay pooled (eviction after a failed execution is node policy; charging them changes STF-8); the end-of-block bisection may re-run a block's contract work up to ~41 times on one anchor (audit finding 3); the contracts layer of the state root still folds all contract state each block (533 ms at 400,000 entries, dev; ~10 blocks of cheap writes) and blocks genesis until it is incremental or contract state is capped.

## 2026-11-14 — ADR-142 amendment 2: the second PR #549 review (Eric 2026-11-14 MCQ, same "cap + price now" decision)

- **Multi-value is bounded and priced:** a function type names at most 16 params and results together, a function holds at most 10,000 calls, and the cold price adds 2,500 per value carried by each function's type, typed block, `if` and call, and 30,000 per value a `loop` carries (typed loops compiled 5.09 ns/fuel at 2,500, their time per value growing with the loops in the function; measured in release, ADR-142 amendment 2). The review's `[1000]->[1000]` blocks compiled 166× slower than their price.
- **A failed call's gas is the same on every validator and counts all its work:** a stack overflow (whose point depends on the ISA) reports its limit plus every page its gas could pay for; any other failure reports what it ran plus its memory, not capped at the limit.
- **Builder work:** a call refused below its upfront price counts `50,000 + len`; one failing contract trial per sender per selection; deployments are shape-checked at admission only after their signature, and refused there above `MAX_COLD_CALL_FUEL`.
- **Not taken:** a separate failed-work budget in selection (the review's proposal). A call is only known to fail by running it, so it doubles the builder's worst case without letting through an honest call the shared budget refuses.
- **Genesis blockers, recorded in BACKLOG.md G1–G3:** free failing calls (needs failed calls included with fee and nonce bump, an STF-8 change: Eric's economics decision), the contracts root fold, and the bisection's re-runs.
## 2026-11-14 — ADR-062a: incremental sparse accounts root built; genesis format unchanged (Eric 2026-11-14 MCQ)

Eric 2026-11-14 MCQ: "incremental root now". Built as an amendment to ADR-062
(`crates/node/src/state/root_cache.rs`): a persistent, local-only (`smt:`) cache of every
sparse-tree node holding two or more accounts (layout superseded by the revision below: branches
only), updated in the same `WriteBatch` as each commit
and revert, withdrawn by any out-of-band account write, checked node by node against a full
rebuild at `Chain::open`. Root value unchanged; `state_root()` stays the full rebuild.

Two findings that contradict the brief, reported rather than worked around (Standing Order 9):

1. The brief said the node "rebuilds the whole sparse root every block". It does not at genesis:
   `STATELESS_ACTIVATION_HEIGHT = u64::MAX`, so the accounts root is the dense address-ordered
   tree, which cannot be incremental at the same value (an insertion shifts every later leaf).
   The cache only works once the sparse tree is active. Activating it at genesis is a separate
   choice for Eric: it changes the genesis root format and retires the dense `account_proof`
   that the light client's SPV path verifies.
2. Record layers (DEX, sealed, governance, staking, ...), channels, contracts and nullifiers are
   dense full rebuilds too and stay so (205-213 ms at 100k records, test profile). Not touched.

### Revision after the PR #540 review (Eric 2026-11-14 MCQ, same item)

- **Branches only.** The first layout stored every node holding two or more accounts,
  single-child levels included: recipient pairs one bit apart cost about 240 entries each, so a
  5 MiB block of them was a storage and slow-block vector once the sparse tree activates
  (review, HIGH, found by reading the code). The cache now stores one entry per branch (N − 1 for
  N accounts, whatever the addresses), with the children's digests, and folds single-child runs in
  memory. Pinned by `neighbour_addresses_cost_one_branch_each_not_a_spine`.
- **One walk a block, and a crossover.** The plan made with the root is carried to the commit in
  the overlay and staged if the write generation is unchanged. A block touching enough keys
  (touched × (bits(N) + 1) × 2 > N, N ≥ 4,096) builds from every account instead. (Crossover
  removed by the second revision below.)
- **A wrong cache no longer stalls a validator.** A declared root that the full rebuild matches
  and the cache does not is accepted on the full rebuild, and the cache is withdrawn; repairs are
  counted (`root_cache_stats`). Debug builds cross-check every planned root.
- **Still dark at genesis, accounts layer only.** Not changed by the revision. The choice stays
  Eric's: (a) activate `FeatureSet::STATELESS` at genesis (genesis root format, `spec/03-state.md`
  rule and vectors for the sparse root, light-client SPV moved off the dense `account_proof`), or
  (b) keep ADR-062a dark and accept that mainnet's per-block accounts cost is unchanged by it.

### Second revision after the PR #540 re-review (Eric 2026-11-14 MCQ, same item)

- **No crossover: a valid cache is always walked.** The crossover's build rewrote all N − 1
  branches plus a range tombstone in the block's batch (about 105 MB at 1M accounts) and was the
  slower path at 1M in its own measurement. The walk is bounded by the union of the touched paths
  and writes only changed branches. A build runs only without a valid cache.
- **Generation moves on entry and exit of every write** (`preview::WriteGuard`), so a plan read
  without the lock mid-write never fits a commit. The chain mutex no longer carries that.
- **Periodic check on the preview path:** `holds()` every 256 heights (`CONFIRM_EVERY`), once per
  height; on a difference the header gets the full rebuild's root and the commit replaces the
  cache. Detection, not prevention, between two checks (ADR-062a, net 3).
- **Debug nodes repair instead of panicking**; only unit tests panic on a cross-check difference.
- **Invariant 24** now names `check_declared_root` and the full rebuild as the reference.
- **Still open, for Eric:** (a) or (b) above. Not decided by this revision. Until (a), the
  brief's per-block cost goal is not delivered on the genesis chain.
## 2026-11-15 — Per-delegation staking records before genesis (Eric 2026-11-14 MCQ)

## 2026-11-19 — ADR-142 amendment 3: failed builder work on its own count; compile prices kept (Eric 2026-11-17 pre-approval, re-freeze bundle 3)

- **Failed contract trials no longer share the applied calls' counter.** Selection moves a failed trial's gas off `Overlay::vm_gas` onto a builder-only count and tries a contract transaction only while applied + failed + its bound fit in `2 × vm.max_block_gas + 1,536,000`. One failing call at the cap no longer leaves the calls after it out. Builder policy only: the block validity rule (GAS-2) is unchanged. This reverses Amendment 2's "not taken": with a two-cap total, one failure at the cap leaves room for every call that passes GAS-2 after it, which a one-cap failure budget did not.
- **Cost, stated:** builder worst case `2 × cap + 3,072,000` per selection; producer ~816 ms of priced work at the 250 M default and ~978 ms at the 300 M ceiling, at 1 ns/unit.
- **Compile prices unchanged.** In release, three runs of the at-limit shapes gave worst 1.231 (loaded), 0.771 and 0.768 ns/fuel, all on `block (type []->[16])` x 2000. The ADR's 1 ns claim now gives the loaded figure. The constants were not tuned to one loaded run.
- **Needs Eric, not decided here:** charging failed calls (STF-8, economics; two failing calls at the cap from two senders still exhaust the builder budget, G1), and whether to lower `VM_GAS_CEILING` given the producer margin at 300 M (G1b).

## 2026-11-21 — ADR-142 amendment 5: the contracts root kept between blocks (BACKLOG G2 CPU side; Eric 2026-11-17 pre-approval of Claude's recommendations)

- **Decided (performance only, root value unchanged):** the contracts layer's leaves stay in
  memory at one write generation and move with every commit and revert by the commit's own batch
  (`state/contracts_root.rs`). A root no longer re-reads and re-hashes all contract state:
  measured (test profile) 617.8 ms before and 78.4 ms after for one new entry over 400,000, and
  0.001 ms for a block that writes no contract entry.
- **Why not the decision brief's A + B here:** a per-block or total contract-state ceiling and a
  per-byte state price change what blocks are valid and what users pay. They are Eric's, so they
  are a separate `[needs-eric]` draft (#644, ADR-142 claim 142d) stacked on this one, re-derived
  against the cached root.
- **Contradicts the earlier WIP (2026-11-19, never pushed):** it said the layer "cannot be made
  incremental byte-identically". The per-entry reading and hashing can be removed without
  touching the value; what stays O(entries) is one pass over the leaves of a positional tree.
  Removing that needs a different layer format (ROOT-5), not done.
- **`check_declared_root` now always asks the full rebuild on a mismatch**, not only after a
  walked accounts plan, so a wrong contracts cache cannot stall a validator. Refusing a bad block
  costs one more full root.
- **An import confirms the caches too (review R-4, confirmed and extended):** `stage_checked`
  takes the same `confirm_due` choice as `preview_root`, so a node that never previews still
  compares both caches with the full rebuild once per 256 heights. R-4 was latent for the dark
  accounts cache and live for this one from genesis. Cost: one full fold per 256 heights on a
  node that only imports.
- **Merge order (#643 review):** #540, then #549, then #643, then #644. #643 is out of the merge
  queue until #540 and #549 are on master; then its base moves to master and it is re-queued.
  The queue now holds any PR whose base is not master.
- **Still open, for Eric:** contract-state growth is unbounded and unpriced (R-1's disk side,
  and the cache's resident memory, not measured).
## 2026-11-21 — PROPOSED, needs Eric: ADR-142 amendment 8, contract-state bounds (`[needs-eric]` draft, stacked on amendment 5)

Not decided. A consensus and economics choice, so it is a draft PR for Eric, not a merge.

- **Proposed rules:** CSTATE-1 caps distinct contract entries a block writes
  (`vm.max_block_contract_writes`, tag 19, default 32,768, `[1,024, 65,536]`; revealed sealed
  actions get a quarter more); CSTATE-2 caps committed contract state counted as key + value +
  1,280 per entry (`vm.max_contract_state_bytes`, tag 20, default 64 MiB, `[8 MiB, 128 MiB]`).
  Only growth past a cap is refused, so a lowered cap never strands a contract. The running
  total is in memory, keyed by the write generation; nothing new is stored.
- **What it bounds after amendment 5** (measured, dev profile, smallest entries): the per-block
  pass over all contract leaves, 11.7 ms at the default (50,686 entries) and 21.0 ms at the
  ceiling (101,372), against 92.2 ms and 179.0 ms for the full fold it replaced.
- **What it does not solve:** contract state is not billed in coin. At the default caps the
  ceiling fills in 2 blocks (smallest entries) to 50 blocks (4 KiB values) for byte fees; after
  that no contract can grow until governance raises it. A per-byte state deposit or fee (decision
  brief 5, option B) is what makes growth paid rather than frozen.
- **Corrects the 2026-11-19 WIP:** it said the layer could not be made incremental; amendment 5
  removed the per-root re-read and re-hash without changing the value.

## 2026-11-21 — ADR-142 amendment 4: re-freeze review R-2/R-5/R-6/R-10 on #549, and the gas caps wait the validity-rule lead (Eric 2026-11-17 pre-approval, re-freeze bundle)

- **R-2 verified, not closed.** One failing call at the cap, pooled and ordered first, no longer keeps honest contract calls out of any block, in the producer and the DAG-BFT paths (two new tests, passed on amendment 3's code). Two failing senders still exhaust the builder budget for free (G1). Charging failed calls is economics and goes to a separate `[needs-eric]` draft PR, not into #549.
- **R-5: scan skipped below the length price, and priced above it.** Measured in release: the shape scan of a 512 KiB module ran at up to 1.116 ns per unit of the old `50,000 + len` charge. A cold call below `deploy_fuel(len)` is now refused before the scan (charge unchanged); one refused after it counts `deploy_fuel(len)` (≤ 0.004 ns per unit measured). A stored cold price was not added: a new state record for a scan that is now priced.
- **R-6: measured and documented, not fixed.** The boundary depends on the build as well as the ISA: one recursion shape completes to depth 10,915 (release) and 10,901 (dev) on the same x86-64 machine. A boundary vector failed across profiles and was dropped; a 5,000-deep vector pins gas short of it. An independent rule needs a module rewriter or a fixed-frame tier, both larger than this amendment: G4, Eric's decision; until then validators run the one published release binary.
- **R-10: text fixed to the code.** GAS-2 now bounds a block at `vm.max_block_gas` plaintext plus `sealed_gas_budget` revealed (1.25 × the cap); the bound comment states 3.25 × for a producer.
- **GAS-5 with #575.** #549 now carries #575; tags 17 and 18 are validity rules, so a passed gas-cap change binds only after `MIN_UPGRADE_LEAD_BLOCKS`, range-checked again at promotion. Pinned by a governance test that votes `vm.max_block_gas` down through the lead.
## 2026-11-14 — Notary (ADR-094) stays a tool and moves to working on its devnet test (Claude, under Eric's approval, Eric 2026-11-14)

Eric 2026-11-14 MCQ, "per-delegation records": ADR-062b is built for the re-genesis instead of
after mainnet (the 2026-10-28 deferral is withdrawn). Choices made while building it (Claude):

- **Same rules, partial state.** The node loads only the records an operation touches and runs
  `maya-staking`'s own function on them. The only new rule entry point is
  `Staking::end_epoch_deferred`, which shares `pay_rewards` with `end_epoch`.
- **Delegator rewards are owed, then paid.** The amounts are exactly the old ones. They arrive on
  the delegation's next touch or through the per-block pass (1,024 a block), not in the boundary
  block. Chosen over F1-style reward accumulators, which would change every delegator's rounding
  and break equivalence with the rules Eric approved.
- **Slash stays eager.** A lazy slash cannot keep each validator's `delegated` exact, because a sum
  of per-delegation floors is not the floor of the sum. Its cost is the offender's own records, a
  key range. Reported as a residual, not hidden.
- **Delegation record ceiling** `1,024 × epoch_blocks / 2`, so a payout backlog cannot outgrow the
  pass. It replaces the 8 MiB record cap (and #522's 1 MiB cap and 16-per-block count, which
  existed only because every staking transaction decoded the whole record).
- **Still Eric's:** a real `min_delegation`. The ceiling bounds the backlog, but dust that fills it
  still refuses new delegation records.

### 2026-11-15 — ADR-062b review round 1 (PR #555, Eric 2026-11-14 MCQ "per-delegation records")

Choices made fixing the first review (Claude; the MCQ decision itself is Eric's):

- **Merged #522 into #555, not rebased** (no force-push). #555 now carries #522's sealed and DEX
  bounds and removes its staking parts: the 1 MiB record cap and its test, `MAX_STAKING_TX_PER_BLOCK`
  and `Overlay::staking_actions`. The 16-per-block count is **removed**, not kept: each staking
  transaction reads its own records, so the 64th in a block costs the 1st
  (`scan_bounds_tests::a_block_of_staking_transactions_costs_each_one_the_same`), and the block's
  byte cap bounds their number. `StateDB::scan_range` uses #522's overlay range
  (`Overlay::records_from`, which `records_under` also uses). Merge order: #522 first, or #555
  alone (it contains #522).
- **Chain-wide ceiling fixed at 131,072 delegation records** (`MAX_DELEGATION_RECORDS`, or the
  half-epoch drain bound on short epochs), down from `1,024 × epoch_blocks / 2` = 1,843,200. The
  state root still folds every staking record each block (ADR-062a covers accounts only), so the
  ceiling is a per-block root cost bound, set near master's 8 MiB record (about 87,000
  delegations). Measured figure: the session report.
- **Per-validator ceiling 16,384** (`MAX_DELEGATIONS_PER_VALIDATOR`, an eighth of the chain's).
  It bounds one slash or downtime boundary, and a dust crowd now has to fill eight validators to
  close delegation for everyone. Top-ups are never refused.
- **Evidence against a tombstoned or unknown validator is refused** after one record read, where
  the rule returned `Ok(empty)`. A public equivocation pair can no longer be replayed into blocks
  that each read the offender's whole key ranges.
- **A delegation record is rewritten only when its meaning changes**, and settling with nothing owed
  leaves `next_payout_epoch` alone: a slash or downtime cut of zero writes nothing.
- **The payout pass resumes after the last payout key** within a block and shares a validator's walk
  between its open payouts.
- **512-bit step in `delegator_share`** (`Amount::mul_div`): kept, with its bound written beside it
  (`earning ≤ total`, so the result is at most `rest`). It is the same arithmetic `end_epoch` used,
  and removing it is the open 512-bit decision (Eric 2026-11-01, `u256-ceiling-no-512`), not this
  PR's.
- **Still Eric's, before the freeze closes:** `min_delegation` as a genesis parameter. At the
  default 10 base units, eight validators' worth of dust (131,072 records) still costs only fees.
  For example 1 coin would make filling the chain-wide ceiling cost 131,072 coins locked.

### 2026-11-15 — ADR-062b review round 2 (PR #555, Eric 2026-11-14 MCQ "per-delegation records")

Choices made fixing the second review (Claude; the MCQ decision itself is Eric's):

- **Ceilings count positions: delegation records plus unbonding entries.** An entry holds its
  position until it is released, so a delegate-and-leave churn from fresh addresses frees nothing;
  before, it grew unbonding entries without bound past both ceilings. Chosen over a budgeted release
  pass, which would change when funds arrive and which entries a slash still reaches. Exits are never
  refused: a full undelegation trades its record for the entry, folding into this epoch's entry adds
  nothing, and an operator's unbond (one entry per release epoch) is exempt. A partial undelegation
  that would open a new entry at a full chain or validator is refused; the owner can still leave in full.
- **Zero entries are deleted.** An entry a slash cuts to zero is removed with its release index
  entry (it credits nothing and the rules' retain ignores it), so counts equal records.
- **A report against an unknown or tombstoned validator is refused before any key read or signature
  check** (two record reads). Under the single-record rules it was a valid no-op; invalid is intended.
- **Validator records are rewritten only when their meaning changes**, as delegations already were.
- **`min_delegation` = 0 is an invalid parameter set** (it admits a zero delegation record).
- **Corrected:** the residual text said `min_delegation` was undecided at 10 base units. The mainnet
  floor is 1 coin (2026-10-30 above); 10 base units is the devnet default.
- **Open for Eric, not accepted on his behalf:** (1) the dust lockout at the 1-coin floor: 16,384
  coins locked close one chosen validator to new delegators, a targeted attack on honest validators
  the per-validator ceiling makes possible; 131,072 coins locked close every new delegation record.
  Options: keep it, a refundable deposit per record, a floor that rises as the ceiling fills, or a
  higher ceiling once the staking root is incremental. (2) PARTIAL per-block cost: the staking
  layer's root folds every record each block (worst case 238.6 ms, test profile), and the boundary
  that releases every position at once took 797.9 ms on the test profile. Genesis with these as
  stated, or an incremental staking root first.

### 2026-11-18 — ADR-062b review round 3 (PR #555, Eric 2026-11-17 pre-approval, re-freeze bundle 3)

- **Tests only, no rule change:** the per-validator ceiling's count of unbonding entries is now pinned
  on epochs long enough that the chain-wide ceiling cannot fire first
  (`unbonding_entries_fill_one_validators_ceiling_and_bound_its_slash`), and the churn test asserts
  the number of staking records the root folds. The new entry counters use checked arithmetic.
- **Corrected:** STK-7 said positions exceed the ceiling "only by operators' own entries" without
  bounding operators. Registration has no record ceiling, so that extra is registered validators ×
  `unbonding_epochs`, and every boundary reads every validator record.
- **Open for Eric, not accepted on his behalf:** (3) the validator count is limited only by
  `min_self_bond` (1,000 coins locked per validator, refundable). Matching the 131,072-position
  ceiling with exempt operator entries alone, at the devnet `unbonding_epochs` = 7, takes 18,725
  validators, 18,725,000 coins locked. Options: keep it, a registration ceiling, or count operator
  entries against the position ceiling (which would let a full chain refuse an operator's unbond).
  Items (1) and (2) above stand: the dust lockout costs 16,384 coins locked for one validator and
  131,072 for the chain at the 1-coin floor.

## 2026-11-13 — Per-block work is bounded by caps, not by how much state exists (Claude, re-freeze bundle)

A sweep of the apply path after PR #500's governance index measured every end-of-block pass against attacker-created records (ADR-062 amendment 2026-11-13, `reports/sessions/2026-11-13-per-block-scan-bounds.md`). Three were fixed with caps that refuse a transaction, never a block, and no key layout change:

- **Sealed mempool:** at most 128 envelopes per reveal height (`MAX_SEALED_PER_REVEAL_HEIGHT`), so a reveal block does what the per-block cap was written to allow. 32,640 at one height cost 18.95 s before.
- **DEX:** the books one block touches may hold 16,384 resting orders between them (`MAX_BOOK_ORDERS_PER_BLOCK`); the first book is always admitted. 16 full books cost 549 ms before.
- **Staking:** the one-value record is capped at 1 MiB (was 8 MiB) and a block carries at most 16 staking transactions, slashing evidence exempt. A full 8 MiB record cost 289 ms every block and 152 ms per staking transaction.
- `merged_records` and two sibling loops read the overlay by range instead of filtering every staged record; an equivalence test pins the output.
- **Not fixed, for Eric:** the state root is O(all state) per block, and accounts cost fees alone to create (ADR-062a or a state-creation price); dust delegations at the default `min_delegation` (10 base units) can fill the staking record; the VM's `gas_limit` is uncapped (`state/context.rs` already says so) and `storage_set` carries no host charge.
## 2026-11-13 — Governance and migration windows sized at 1 block/s (re-freeze bundle, Eric)

The chain makes about one block a second (`docs/block-time.md`), and four compiled windows were sized at the 15 s PoW spacing, so each was 15x shorter in wall-clock than its comment said. Eric approved re-sizing them in the re-freeze bundle (PR #500):

- **`MIN_VOTING_BLOCKS`** 480 -> 7,200 (2 h) and **`MIN_TIMELOCK_BLOCKS`** 960 -> 14,400 (4 h), `crates/governance/src/limits.rs` (invariant 12).
- **`MIN_MIGRATION_WINDOW`** 172,800 -> 2,592,000 (30 d) and **`MIN_EMERGENCY_WINDOW`** 17,280 -> 259,200 (3 d), `crates/crypto-pq/src/agility/mod.rs`. This replaces the 2026-11-08 "2-day migration window stays".
- **Council sunset lead** follows, being derived: 1,172,800 -> 3,592,000 blocks (about 41.6 d), replacing the 13.6 d recorded on 2026-11-08 (ADR-082 H3).
- **The rate is pinned:** a genesis whose chain id contains `mainnet` (case and surrounding space ignored, the genesis ceremony's rule; `maya2c-mainnet` included, independent AI review R-9) must set `bft.round_interval_ms = 500`, refused otherwise by `GenesisConfig::validate` and `cargo xtask genesis-check`.
- **Not changed, flagged:** `MAX_VOTING_BLOCKS`, `MAX_TIMELOCK_BLOCKS` and `EXECUTION_GRACE_BLOCKS` are still 15 s-sized (27.8 h, 55.6 h, 27.8 h at 1 s). They are ceilings, so not a safety hole; left for Eric. (Superseded the same day: see "Eric: governance caps x15" below.)

## 2026-11-13 — Eric: governance caps x15 (re-freeze bundle, PR #500)

Eric chose (MCQ, recommended option) to scale the three governance ceilings x15 so they keep the wall-clock durations their 15 s sizes meant, now that the chain makes one block a second. `crates/governance/src/limits.rs`:

- **`MAX_VOTING_BLOCKS`** 100,000 -> 1,500,000 (about 17.4 d).
- **`MAX_TIMELOCK_BLOCKS`** 200,000 -> 3,000,000 (about 34.7 d).
- **`EXECUTION_GRACE_BLOCKS`** 100,000 -> 1,500,000 (about 17.4 d).

Floors unchanged and still under their ceilings (7,200 <= 1,500,000; 14,400 <= 3,000,000; the `const` block in `limits_tests.rs` fails the build otherwise). The schedule horizon (`MAX_SCHEDULE_HORIZON_BLOCKS`, 31,536,000) is measured from `expires_after`, so a longer grace does not shrink it at proposal time; the stake-lock requirement is `executable_from`, unaffected by grace. All schedule arithmetic is `saturating_add` on `u64`. Pinned by `the_ceilings_keep_their_15_second_era_durations_at_one_block_per_second`.

## 2026-11-13 — The governance settlement pass reads only due proposals (re-freeze bundle, PR #500)

The end-of-block pass read every proposal record ever written, settled ones included, on every block. Measured on the test profile: 286 ms a block at 100,000 records, linear at about 2.9 µs a record (`reports/sessions/2026-11-13-governance-settle-index.md`). An attacker could reach that in 3,125 blocks for fees alone: the deposit is refundable, nothing limits proposals per account, and settled records stayed in the scan. Fix, recorded as an ADR-082 amendment: a settlement index `g:due:<height BE><id>` under the existing governance layer (no new prefix, invariant 25 unchanged). `propose` writes keys at `voting_closes + 1` and `executable_from`, at most `MAX_PROPOSALS_DUE_PER_HEIGHT` = 64 per height, and refuses a proposal that would overflow one. The pass consumes keys at or below its height in proposal-identifier order. Rejected: a cap on open proposals (an attacker holding it blocks governance for refundable capital) and a per-block budget with carry-over (it shifts when proposals finalize, and a flood delays honest ones). It changes the governance layer's root whenever a proposal exists, which is why it lands before genesis. Pinned by `the_pass_reads_no_proposal_that_is_not_due`, `the_indexed_pass_matches_the_full_scan` and `a_height_takes_at_most_the_capped_number_of_due_proposals`.
## 2026-11-15 — Block-validity parameters wait a full upgrade lead; 15 s-era constants rescaled to 1 block/s (Eric 2026-11-14 MCQ)

Eric chose "all format fixes" in the 2026-11-14 re-freeze MCQ. Two of them land together (ADR-082 amendment 2026-11-15):

- **Validity parameters through the schedule.** A passed change to a `ParameterKey` that decides block validity (tags 2-8: DEX fill and order ceilings, oracle and shielded per-block caps, VM memory/module/default-gas bounds) no longer lands at execution. It waits in `g:pending:<tag>` and is in force from execution + `MIN_UPGRADE_LEAD_BLOCKS` (30 days), the lead a protocol upgrade carries, which is what the 2026-10-28 entry's "parameters change by protocol upgrade at an activation height, not by a live governance knob" asked for. The protocol fee, proposal deposit and default suite still apply at execution. One change waits per key; a later passed change to the same key replaces it. The lead is compiled, not chosen by the proposal, so the transaction format does not change.
- **Rescale:** `WORK_HALF_LIFE_BLOCKS` 5,760 -> 86,400 (one day), DAG `EPOCH_LENGTH` 30,000 -> 450,000 (5.2 days; `DAG_ACTIVATION_HEIGHT` follows it), smart-account `BLOCKS_PER_DAY` 43,200 -> 86,400 (ADR-086 F7 closed), PQ transport `ROTATION_INTERVAL_BLOCKS` 1,000 -> 15,000 with the 15,000 s fallback unchanged. `TARGET_BLOCK_TIME` stays 15 and is documented as PoW-only. The governance windows, migration windows and the round-interval pin are in PR #500 (open).
- **Not rescaled:** `PRUNE_DEPTH` (node-local policy, was defined as `EPOCH_LENGTH`) is now its own constant and stays 30,000 (8.3 h at 1 block/s). Following the epoch would have raised a default pruned node's worst case to ~2.1 TiB at the 5 MiB block cap, and the default `--bootstrap-from` and snapshot-serve depths, which are `PRUNE_DEPTH`, would have served no snapshot for the first 5.2 days. The `get_balance_at_height` bound `MAX_BALANCE_REWIND` (was `PRUNE_DEPTH`) is its own 30,000, so one public call never walks 450,000 heights. Both pinned by unit tests.
- **Settle-time range check:** a passed proposal whose value fails `ParameterChange::validate` at execution (a release tightened the bound mid-vote) now expires instead of failing every candidate block, matching the promotion pass.
- **Review fixes (2026-11-16):** the spec rule is CON-17, not CON-16 (#573 holds CON-16 for the header hash id; claimed in adr-claims.txt). Execution writes `g:params` only when a key that applies at once changed, the rule promotion already follows, so a validity-only or upgrade-only proposal no longer materialises an unchanged table. Kept as they are, with the reason in ADR-082: `MAX_LINKED_GAP` stays 50,000 (13.9 h at 1 block/s, node-local, the byte cap binds), and the vote lock stays at execution for validity changes, as for upgrades. #549's `VmMaxTxGas`/`VmMaxBlockGas` must be classified as validity rules when the two PRs meet.
- Consequence for operators: default pruning, bootstrap and snapshot depths are unchanged. **This binary is not a rolling upgrade:** `dag_epoch()` divides by `EPOCH_LENGTH`, so old and new binaries hash mining shares against different DAG caches from height 30,000. It needs the planned re-genesis. Activating work claims later must re-check the work-to-stake weight ratio, since steady-state work credit grows with the 15x longer half-life.
## 2026-11-14 — Contract gas is capped per transaction and per block, and native work is priced (Eric 2026-11-14 MCQ, ADR-142)

Eric chose "cap + price now" in the 2026-11-14 MCQ. A call's `gas_limit` was sender-chosen up to `u64::MAX`, nothing bounded a block's contract work, host functions and the per-call module compile were unmetered, and every call loaded the contract's whole storage. Now:

- **Caps:** `vm.max_tx_gas` (tag 17) and `vm.max_block_gas` (tag 18), 250 M each, governed within compiled bounds `[239,979,600, 500,000,000]`; a call is capped at the smaller, a block's calls and deployments share the block cap. Refused at admission and in block validation; the builder leaves what does not fit for a later block.
- **Prices** (`crates/vm/src/gas.rs`), at about 1 ns of measured worst-case native work per unit: 50,000 per call, 450 per module byte on a contract's first call in a block (the compile) and 1 after, 600 per host call, storage reads 5,000 + 1/byte, writes 5,000 + 240/byte (priced by persistence: ≤ ~1 MiB of new state per block), events 600 + 1/byte. No refunds.
- **Lazy storage:** the host reads committed storage one key at a time; randomized equivalence against the old whole-storage load is a test.
- **Not decided here, left open:** a fee per gas (economics, Eric's), charging failed plaintext calls (changes STF-8), and an incremental contracts root (the root still folds all contract state each block).

## 2026-11-14 — ADR-142 amended after the PR #549 review (Eric 2026-11-14 MCQ, same "cap + price now" decision)

The review found work running below its price or outside the block's budget; measured on this PC, the worst was a module of 200 functions with 50,000 locals each that compiled in 678 ms (release) against a price of 0.78 M gas, and 40,000 `if`s in one function that compiled in 10.4 s. Implementing "cap + price now" fully therefore needed:

- **Compile priced by shape** (`crates/vm/src/shape.rs`): a linear pre-pass before any compile; at most 1,024 locals and 2,000 control operators per function and 10,000 functions; cold price adds 125,000 per function, 170 per local, 30,000 per control operator, 8,000 per call; a module whose cold price exceeds 235,979,600 is refused at deployment, so the governance floor still keeps every deployed module callable.
- **Linear memory priced** at 6,000 a page held (GAS-7), and a call may hold only what its gas pays for.
- **Refused host calls trap**, and event topics are capped at 256 bytes.
- **Failed work stays counted:** a failed call spends the gas it used (not its limit) and a failed deployment its price, counted before the compile, in block selection and in revealed sealed actions.
- **Revealed sealed actions get their own budget**, a quarter of `vm.max_block_gas`, so a plaintext call cannot void a due sealed call.
- **Warm and cold storage reads:** the block's first read of a committed key pays 95,000 more (measured 73–113 µs from disk with direct I/O). A bloom filter was measured and left out (it made hits slower with a small cache).
- **No per-call copies** of the block's staged contract storage; contract trials no longer clone the overlay.
- **Ceiling 300 M** for both caps (was 500 M).

Raised with Eric, not decided here: one gas-filling call per block crowds out other contract calls for a byte fee (fee per gas or a per-sender share is economics); failing calls are free and stay pooled (eviction after a failed execution is node policy; charging them changes STF-8); the end-of-block bisection may re-run a block's contract work up to ~41 times on one anchor (audit finding 3); the contracts layer of the state root still folds all contract state each block (533 ms at 400,000 entries, dev; ~10 blocks of cheap writes) and blocks genesis until it is incremental or contract state is capped.

## 2026-11-14 — ADR-142 amendment 2: the second PR #549 review (Eric 2026-11-14 MCQ, same "cap + price now" decision)

- **Multi-value is bounded and priced:** a function type names at most 16 params and results together, a function holds at most 10,000 calls, and the cold price adds 2,500 per value carried by each function's type, typed block, `if` and call, and 30,000 per value a `loop` carries (typed loops compiled 5.09 ns/fuel at 2,500, their time per value growing with the loops in the function; measured in release, ADR-142 amendment 2). The review's `[1000]->[1000]` blocks compiled 166× slower than their price.
- **A failed call's gas is the same on every validator and counts all its work:** a stack overflow (whose point depends on the ISA) reports its limit plus every page its gas could pay for; any other failure reports what it ran plus its memory, not capped at the limit.
- **Builder work:** a call refused below its upfront price counts `50,000 + len`; one failing contract trial per sender per selection; deployments are shape-checked at admission only after their signature, and refused there above `MAX_COLD_CALL_FUEL`.
- **Not taken:** a separate failed-work budget in selection (the review's proposal). A call is only known to fail by running it, so it doubles the builder's worst case without letting through an honest call the shared budget refuses.
- **Genesis blockers, recorded in BACKLOG.md G1–G3:** free failing calls (needs failed calls included with fee and nonce bump, an STF-8 change: Eric's economics decision), the contracts root fold, and the bisection's re-runs.
## 2026-11-15 — Work-kind byte on shares and claims; work claims refused by a compiled guard (Eric 2026-11-14 MCQ)

## 2026-11-19 — ADR-142 amendment 3: failed builder work on its own count; compile prices kept (Eric 2026-11-17 pre-approval, re-freeze bundle 3)

- **Failed contract trials no longer share the applied calls' counter.** Selection moves a failed trial's gas off `Overlay::vm_gas` onto a builder-only count and tries a contract transaction only while applied + failed + its bound fit in `2 × vm.max_block_gas + 1,536,000`. One failing call at the cap no longer leaves the calls after it out. Builder policy only: the block validity rule (GAS-2) is unchanged. This reverses Amendment 2's "not taken": with a two-cap total, one failure at the cap leaves room for every call that passes GAS-2 after it, which a one-cap failure budget did not.
- **Cost, stated:** builder worst case `2 × cap + 3,072,000` per selection; producer ~816 ms of priced work at the 250 M default and ~978 ms at the 300 M ceiling, at 1 ns/unit.
- **Compile prices unchanged.** In release, three runs of the at-limit shapes gave worst 1.231 (loaded), 0.771 and 0.768 ns/fuel, all on `block (type []->[16])` x 2000. The ADR's 1 ns claim now gives the loaded figure. The constants were not tuned to one loaded run.
- **Needs Eric, not decided here:** charging failed calls (STF-8, economics; two failing calls at the cap from two senders still exhaust the builder budget, G1), and whether to lower `VM_GAS_CEILING` given the producer margin at 300 M (G1b).

## 2026-11-21 — ADR-142 amendment 4: re-freeze review R-2/R-5/R-6/R-10 on #549, and the gas caps wait the validity-rule lead (Eric 2026-11-17 pre-approval, re-freeze bundle)

- **R-2 verified, not closed.** One failing call at the cap, pooled and ordered first, no longer keeps honest contract calls out of any block, in the producer and the DAG-BFT paths (two new tests, passed on amendment 3's code). Two failing senders still exhaust the builder budget for free (G1). Charging failed calls is economics and goes to a separate `[needs-eric]` draft PR, not into #549.
- **R-5: scan skipped below the length price, and priced above it.** Measured in release: the shape scan of a 512 KiB module ran at up to 1.116 ns per unit of the old `50,000 + len` charge. A cold call below `deploy_fuel(len)` is now refused before the scan (charge unchanged); one refused after it counts `deploy_fuel(len)` (≤ 0.004 ns per unit measured). A stored cold price was not added: a new state record for a scan that is now priced.
- **R-6: measured and documented, not fixed.** The boundary depends on the build as well as the ISA: one recursion shape completes to depth 10,915 (release) and 10,901 (dev) on the same x86-64 machine. A boundary vector failed across profiles and was dropped; a 5,000-deep vector pins gas short of it. An independent rule needs a module rewriter or a fixed-frame tier, both larger than this amendment: G4, Eric's decision; until then validators run the one published release binary.
- **R-10: text fixed to the code.** GAS-2 now bounds a block at `vm.max_block_gas` plaintext plus `sealed_gas_budget` revealed (1.25 × the cap); the bound comment states 3.25 × for a producer.
- **GAS-5 with #575.** #549 now carries #575; tags 17 and 18 are validity rules, so a passed gas-cap change binds only after `MIN_UPGRADE_LEAD_BLOCKS`, range-checked again at promotion. Pinned by a governance test that votes `vm.max_block_gas` down through the lead.

## 2026-11-22 — ADR-142 amendment 7: x86-64-v2 code generation, and one node's engine faults are not failed calls (Eric 2026-11-17 pre-approval, re-freeze bundle)

- **ISA level measured, then pinned.** Float rounding runs at 0.52–0.67 ns per unit of gas under every Cranelift configuration (host-inferred, bare baseline, x86-64-v2), four to five times the `i64.div_u` loop the block-time derivation cited; the worst, 0.674 on the baseline, is about 211 ms at GAS-2's 1.25 × bound, inside 1 s, so nothing is repriced. The engine compiles at x86-64-v2 on x86-64 (popcnt 3x faster than the baseline, no float libcalls into each binary's C runtime) and refuses a CPU without it at start-up. Gas unchanged. One `unsafe` call (`cranelift_flag_enable`) with a SAFETY comment; needs a reviewer.
- **Local engine faults by type, not "not a trap".** An `io::Error` or wasmtime `OutOfMemory` becomes `VmError::LocalFault`, mapped to `NodeError::Storage`, so selection stops and apply refuses instead of recording a failed call (#647) or leaving the transaction out on one node (GAS-9). The review's "anything not a `Trap`" rule was rejected: a declared table fails instantiation with a plain error on every node, and would have stopped block building everywhere.
- **Recorded, not changed:** every module with a table fails every call (the limiter refuses tables); alike on every node, a contract-ABI question.

## 2026-11-22 — ADR-142 amendment 6: deployment scans counted, block selection is consensus (GAS-9), the stack boundary belongs to the binary (Eric 2026-11-17 pre-approval, re-freeze bundle)

- **Deployments are scanned only when the scan counts.** Refused on `deploy_fuel(len)` before the scan; any refusal after the scan counts `deploy_fuel(len)`, as a cold call does; block selection bounds a deployment by what the block has left, not by a scan. Four red-then-green tests.
- **The builder budget is GAS-9, block derivation.** Under DAG-BFT every validator runs the selection, so `BUILDER_GAS_CAPS`, `BUILDER_GAS_SLACK` and one-failure-per-sender are consensus, changed only by a hard fork. Four `builder_select` vectors pin them; CON-12 and CON-13 point to GAS-9.
- **Cranelift's target is named, so the host CPU no longer sets ISA flags.** Gas unchanged; pinned by a test that was red under inference.
- **R-6 restated:** release builds of one source on one CPU overflow at depth 10,915 (x86-64 Windows) and 10,919 (x86-64 Linux), so "x86-64 only" is not enough; the condition is one published release binary for every validator, which nothing enforces. BACKLOG G4 is a genesis blocker with three options for Eric (one binary by runbook, a deterministic call-depth bound as parity's `wasm-instrument` stack limiter, or the Pulley tier).

## 2026-11-14 — Notary (ADR-094) stays a tool and moves to working on its devnet test (Claude, under Eric's approval, Eric 2026-11-14)

- **Tool only:** the notary is `maya2c notary` on the existing `AnchorAttestation` payload (tag 34, schema `notary-v1`). No consensus, wire or state change; no node RPC added.
- **Status:** `features.toml` `notary` moves from stub to working on `bins/maya2c-cli/tests/notary_devnet_tests.rs` (a stamp lands in a block on a dev node, the receipt matches that block, one changed byte is refused). Class stays RESEARCH: no consensus path calls it.
- **Verify hardening:** verify checks each served block's body against the `tx_root` in the header its id hashes (not the separate JSON field), checks the id is the one named, refuses a non-contiguous batch, and checks the stamp's signature.
- **Shortcuts kept:** finality is the node's checkpoint with committee signatures unverified, and the chain tag comes from the node's `get_chain_info`.
- **Open anomaly:** one early manual run never saw the stamp land; it was not reproduced and its cause is unknown.
## 2026-11-15 — Block-validity parameters wait a full upgrade lead; 15 s-era constants rescaled to 1 block/s (Eric 2026-11-14 MCQ)

Eric 2026-11-14 MCQ, "all format fixes", applied before the re-genesis. ADR-044 amendment 2026-11-15.

- **`WorkKind` byte:** `MiningShare` and `WorkClaim` now start with a closed `#[repr(u8)]` kind byte. `0 = DagPow` is the only kind. Any other byte is a decode error (spec MINE-11). This is a wire change for both payloads (share 16 → 17 bytes, claim 32 → 33), so it lands only with the re-genesis.
- **Registry:** new kinds come only from a new binary, gated by a new feature bit and a scheduled activation (ADR-082). Neither governance nor genesis can add one. ADR-079's `kind = 1` and ADR-129's `ClaimCompute` are both still open.
- **`WORK_CLAIM_ADR_ACCEPTED = false`:** while it is false, `FeatureSet::WORK_CLAIM` is outside `FeatureSet::IMPLEMENTED` (the zkML model of PR #506). A genesis naming it is refused at boot; a governance entry naming it is admitted, written, and halts this binary at its activation height; `features().work_claim` stays `u64::MAX`. The mempool refuses `TxKind::ClaimWork`. The executor's existing refusal (`WorkClaimInactive` below `u64::MAX`) is unchanged. Flipping the flag is part of the change that lands an Accepted ADR binding a claim to a verified share.
- **Why not refuse at the vote (review of #570, 2026-11-15):** the first version refused the entry in `UpgradeSchedule::new`. A later binary with the flag set would admit the same entry, and this binary would refuse the block carrying the vote: a fork at the vote, which #506 removes for zkML. The brief said "refuse scheduling"; that wording conflicts with #506's halt model, and this is reported rather than worked around. What is refused is genesis scheduling (at boot) and the claim itself (mempool, executor); a governance entry halts instead. #570 merges #506 into its branch, so #506 merges first.
- **Kind namespaces:** the share header and spent-share key do not commit to the kind. Rather than change the share header now, any new kind must use its own header domain and spent-key prefix (MINE-11, ADR-044 amendment); both executors destructure the kind irrefutably so a new variant fails to compile until its gate is written.
- **Open for Eric (third review of #570, 2026-11-15):** the MCQ approved "refuse scheduling"; what ships refuses a genesis entry and halts at activation for a governance or council entry. In practice a passed vote naming bit `0x10` halts the whole network at its activation height, as scheduling an unsupported version already does. Eric has not yet acknowledged this reading; it needs his explicit OK before #570 merges, and the merge order stays #506, then #570. Also declined in that review: mixing the kind into MINE-9's share header and spent-share key now. It would change MINE-9's vectors and every miner for a kind that does not exist; the irrefutable destructure plus MINE-11's own-domain rule keep kinds apart, and a new kind's binary adds its domain then.
## 2026-11-15 — Signatures name their wire version and suite; headers reserve a hash id (Eric 2026-11-14 MCQ, ADR-143)

Eric chose "all format fixes" in the 2026-11-14 MCQ. Two of them land here, before genesis, because after it either would be a fork.

- **Signed header (TX-6):** every transaction signature now covers `domain ‖ chain_tag ‖ wire_version ‖ suite ‖ body`. The suite is `0x30` for v5/v6, the frame's suite for v7, and `0x00` for v8 (each multisig key names its own suite in the signed policy). The domains moved to `tx.v5`, `tx.suite.v3` and `tx.multisig.v3`. No replay was possible before: domains and the payload tag kept v5 to v8 apart. What changes is that a later version reusing a domain cannot open one by accident. Every signature, and every real txid, changes; re-genesis is planned.
- **Header hash id (CON-16):** the high nibble of the header's version byte names the hash used for the block id, the PoW seed, `tx_root` and `state_root`. 0 is BLAKE3, and any other value is refused. No byte was added, because a 146th header byte would have touched the XDP relay, the GPU and pool miners, Stratum V2 and the light client. Txids and addresses get no byte of their own: the signed wire version and suite already select their hash, and adding a new one of either is already a scheduled upgrade.
## 2026-11-16 — Genesis records and public wording made consistent (Eric 2026-11-14 MCQ)

Eric 2026-11-14 MCQ, answers (2), (3) and (4). PR "[re-freeze] genesis records and public wording".

- **Validator floor 7:** `genesis-ceremony` refused fewer than 4 validators on a value-bearing chain while `genesis-check` refused fewer than 7, so the ceremony could write a genesis its own check rejected. `MIN_LAUNCH_VALIDATORS` is now 7 (`bins/genesis-ceremony/src/committee.rs`), pinned by `mainnet_needs_seven_validators_a_testnet_does_not`. ADR-047 amendment 2026-11-16. The node accepts any committee size, as before.
- **Supply headline 19,687,500:** the coins the four halving eras issue, as coded (`the_mainnet_halving_era_pays_19_687_500_coins_then_the_tail_from_epoch_140256`). The tail after those eras and the 7,000 coins of genesis bonds are shown as separate figures. Older records that say 21M are marked superseded above, not deleted.
- **Burn:** spec FEE-1 (v0.1.1) and the econ SIM now state the recorded 100% base-fee burn (ADR-029; B-16 above). The split function is **not** deleted: it is called by the econ SIM, `spec-ref`, a Kani harness, the Lean and Z3 proofs and the FEE-1 vectors, so it is not dead code, and deleting it would lower `cargo xtask proof-ratchet`. The node never calls it (`FeesGenesis::record` fixes `treasury_bps: 0`). This closes the B-16 open item.
- **Public sentence:** "Pre-audit: no external audit yet; validators run by one operator at launch; external audit planned after funding." On the site (FAQ, launch ladder), README, SECURITY.md, MISSION.md, EMPIRE.md, ADR-042, LAUNCH.md, the announcement draft, the exchange notes and the OSTIF application. It replaces "external audit pending".
- **Review follow-up (same PR):** a value-bearing ceremony without `--validators` is now refused, since it would skip the floor and write a proof-of-work genesis (`a_mainnet_without_a_committee_is_refused_and_a_testnet_may_omit_one`); the node still has no runtime floor. `spec/tests/units.json` no longer publishes a 21M cap: it carries `halving_era_issuance_coins` 19,687,500 and the exact schedule sum in base units (pinned by `coin_constants_match_the_spec_vector`). The site's `/v1/supply` example now shows the real genesis shape (reserve plus bonds, about 1.23 × 10^17 coins) and says the reserve is counted as circulating until a later node release. Testnet copy says it is paused. The headline is worded as a schedule total that is at most paid. Follow-up: `harden-host.ps1` still checks only v2..v4 until the mainnet start script fixes v5..v7's ports.
- **Review round 2 (same answers):** a value-bearing run without `--validators` is refused in `parse_args` (`check_launch_args`), before any directory or key is written, and both `run()` and `assemble` read the committee and council before creating the out dir. The ceremony and foundation-validator runbooks, the explorer HUD (seven validators drawn) and the econ SIM's validator floor (7) follow the floor. The Hub's testnet chip reads "Built (testnet paused)". The spec README now records the FEE-1 and spec 9 in-place corrections as a dated note only; the earlier general rule (pre-genesis spec corrections without a MIP) is withdrawn, because no decision of Eric's covers it. FEE-1 is conditioned on staking (without `bft.staking` the tip stays in the collector), and spec 4's Gaps says the routing is state-level and that the tip-to-pool step has no test yet.

## 2026-11-17 — Re-freeze bundle integration: merge order and conflict resolutions (Eric 2026-11-14 MCQ)

Branch `refreeze/integration` merges every re-freeze PR onto master to prove they compose; it is a check, not a merge path.
- **Order:** #540 (state root) → #555 (carries #522) → #500 → #575 → #549 → #570 (carries #506) → #573 → #598. #520 was already on master.
- **#555 × #540, ADR-062:** both appended amendments; kept both in date order (2026-11-13, 2026-11-14, 2026-11-15) and noted under the 2026-11-13 "root not fixed" paragraph that 062a is accounts-only and dark at genesis.
- **#575 × #500, `governance_exec.rs`:** #500 split settlement into `settle_one` + `admitted` over the `g:due:` index; #575 edited the old loop body. Kept #500's structure and moved #575's two changes into it: `ParameterChange::validate` in `admitted` (an out-of-range value expires the proposal, never fails the block) and `apply_changes(.., height)` (validity keys go to `g:pending:`). Both prefixes (`g:due:`, `g:pending:`) kept; `activate_parameters` still runs after `settle_governance`.
- **#549 × #575, `ParameterKey::is_validity_rule`:** the exhaustive match refused #549's tags 17 and 18 (E0004). Classified both as validity rules (a gas cap refuses a call or a block, GAS-1/GAS-2), so a passed cap change waits `VALIDITY_RULE_LEAD_BLOCKS`; the test list is `[2..8, 17, 18]`. ADR-082 had said the second-merged PR does exactly this.
- **#570 × #575:** spec rules kept in number order (CON-14, 15, 16, 17), invariant 34 keeps both paragraphs, test imports unioned.
- **#598 × master (#595):** ceremony test imports and tests unioned; spec README keeps `09-mining` 0.1.1 and the `10-vm-gas` row.
- **Validity parameters through the schedule.** A passed change to a `ParameterKey` that decides block validity (tags 2-8: DEX fill and order ceilings, oracle and shielded per-block caps, VM memory/module/default-gas bounds) no longer lands at execution. It waits in `g:pending:<tag>` and is in force from execution + `MIN_UPGRADE_LEAD_BLOCKS` (30 days), the lead a protocol upgrade carries, which is what the 2026-10-28 entry's "parameters change by protocol upgrade at an activation height, not by a live governance knob" asked for. The protocol fee, proposal deposit and default suite still apply at execution. One change waits per key; a later passed change to the same key replaces it. The lead is compiled, not chosen by the proposal, so the transaction format does not change.
- **Rescale:** `WORK_HALF_LIFE_BLOCKS` 5,760 -> 86,400 (one day), DAG `EPOCH_LENGTH` 30,000 -> 450,000 (5.2 days; `DAG_ACTIVATION_HEIGHT` follows it), smart-account `BLOCKS_PER_DAY` 43,200 -> 86,400 (ADR-086 F7 closed), PQ transport `ROTATION_INTERVAL_BLOCKS` 1,000 -> 15,000 with the 15,000 s fallback unchanged. `TARGET_BLOCK_TIME` stays 15 and is documented as PoW-only. The governance windows, migration windows and the round-interval pin are in PR #500 (open).
- **Not rescaled:** `PRUNE_DEPTH` (node-local policy, was defined as `EPOCH_LENGTH`) is now its own constant and stays 30,000 (8.3 h at 1 block/s). Following the epoch would have raised a default pruned node's worst case to ~2.1 TiB at the 5 MiB block cap, and the default `--bootstrap-from` and snapshot-serve depths, which are `PRUNE_DEPTH`, would have served no snapshot for the first 5.2 days. The `get_balance_at_height` bound `MAX_BALANCE_REWIND` (was `PRUNE_DEPTH`) is its own 30,000, so one public call never walks 450,000 heights. Both pinned by unit tests.
- **Settle-time range check:** a passed proposal whose value fails `ParameterChange::validate` at execution (a release tightened the bound mid-vote) now expires instead of failing every candidate block, matching the promotion pass.
- **Review fixes (2026-11-16):** the spec rule is CON-17, not CON-16 (#573 holds CON-16 for the header hash id; claimed in adr-claims.txt). Execution writes `g:params` only when a key that applies at once changed, the rule promotion already follows, so a validity-only or upgrade-only proposal no longer materialises an unchanged table. Kept as they are, with the reason in ADR-082: `MAX_LINKED_GAP` stays 50,000 (13.9 h at 1 block/s, node-local, the byte cap binds), and the vote lock stays at execution for validity changes, as for upgrades. #549's `VmMaxTxGas`/`VmMaxBlockGas` must be classified as validity rules when the two PRs meet.
- Consequence for operators: default pruning, bootstrap and snapshot depths are unchanged. **This binary is not a rolling upgrade:** `dag_epoch()` divides by `EPOCH_LENGTH`, so old and new binaries hash mining shares against different DAG caches from height 30,000. It needs the planned re-genesis. Activating work claims later must re-check the work-to-stake weight ratio, since steady-state work credit grows with the 15x longer half-life.

## 2026-11-21 — Wasmtime 48.0.10 rides the mainnet genesis; gas pinned to golden vectors (Claude, under Eric 2026-11-17 pre-approval)

- **Why now:** RUSTSEC-2026-0566..0573 (fixed in 48.0.9) turned `cargo deny` red on master. 48.0.10 is the newest 48.x; its one further fix is in `wasmtime-wasi`, which the VM does not link. The pin is consensus-critical, and genesis is a fresh chain, so the bump costs no extra fork.
- **Reachability:** none of the eight reaches a contract here. Three are in `wasmtime-wasi`/`wasmtime-wasi-http` or the component model, not linked. The two GC-heap ones need GC objects, which a build without the `gc` feature cannot create. The `threads` one needs the threads proposal turned on, and the config never does. The no-`gc` one needs a runtime check against a non-final function type; the VM refuses every table, and the cast instructions need reference types, which are off (`vm_tests.rs` pins both). `Caller::get_module_export` is not called.
- **Gas evidence:** a `diff -r` of the registry sources (48.0.8 vs 48.0.10) shows one codegen change: 48.0.9's GC-object field address computation in `wasmtime-internal-cranelift` (`bounds_checks.rs`, `func_environ/gc.rs`). Linear-memory accesses pass `None` for the new backwards offset, so contract IR is identical. The `wasmtime` crate also caps non-moving memory growth at `memory_reservation`, unreachable with `MAX_MEMORY_PAGES` = 256 (16 MiB). `cranelift-codegen`, `cranelift-frontend` and `pulley-interpreter` are byte-identical, and no changed line in any wasmtime crate mentions fuel. `crates/vm/tests/gas_golden_tests.rs` was recorded on 48.0.8 and passes unchanged on 48.0.10. It pins gas, the smallest sufficient gas limit and the output of seven modules.
- **Locks:** the root, `fuzz/`, `offsec-sandbox/` and `apps/wallet-gui/src-tauri/` locks change only the 25 wasmtime, cranelift and pulley packages (version and checksum). `cargo update` also rewrote unrelated dependency edges in the wallet lock, so that lock was spliced instead. `cargo metadata --locked` passes in all four.
## 2026-11-17 — PR #577 narrowed to the wasmtime pin; host-surface bounds left to #549 (Claude, under Eric 2026-11-17 pre-approval)

- **Kept:** `WASMTIME_VERSION` corrected to the pinned `48.0.8`; the pin test now reads `crates/vm/Cargo.toml` and `Cargo.lock` instead of comparing the constant with itself; `config_digest` hashes the constant instead of a stale `48.0.1` literal (module-cache key only, held in memory, never in a state root).
- **Dropped from #577, because #549 (ADR-142) already does them:** sizes checked before guest copies, an event topic cap (#549 sets 256 bytes; #577 had proposed 128), and one contract VM per `StateDB`. Two values for one consensus cap would contradict each other, so #549's stands.
- **Dropped:** the comment rewrites in `runtime.rs`, `host.rs`, `vm_exec.rs` and `contracts.rs`. #549 rewrites those functions (host failures trap, storage reads lazily), so the rewritten comments would be wrong once it merges.
## 2026-11-17 — State and fee rule hardening joins re-freeze bundle 3 (Claude, under Eric 2026-11-17 pre-approval)

Eric pre-approved Claude's recommendation (2026-11-17) to move PR #584 into re-freeze bundle 3. Recorded in [ADR-144](docs/adr/ADR-144-state-and-fee-rule-hardening.md).

- **Fee-free shares carry nothing (spec FEE-7):** a mining share is exempt from the base fee only with no inputs and no outputs; one predicate (`state::fee_exec::is_free_share`) serves the executor and the mempool. No fee level or parameter changes.
- **Governed DEX rate:** a pool that cannot carry the governed protocol rate refunds its batch, and a route through it is a no-op, instead of failing the block (invariant 7); book fills take the governed protocol rate and never the LP rate, as `docs/dex.md` states (invariant 17), so a book is priced on the protocol rate alone and keeps matching. The governed default is 0 bps, so genesis behaviour is unchanged.
- **FEE-7 vectors** now include refused cases (`share_fee`, `FeeTooLow`) replayed against the node's `required_fee`; the rule is no longer tagged positive-only. A fee-free share's auth bytes stay bounded by `MAX_SIGNERS` and the share and block caps, not by the fee (recorded in ADR-144).
- **Confidential-AI** participants refuse a second share set and a second masked input per instance (RESEARCH crate, not consensus).
- **Dropped from #584 as duplicates or conflicts:** the settlement range recheck is carried by #575; the work-claim schedule refusal conflicts with #570/#506, which make unimplemented bits halt at activation rather than refuse at construction.
## 2026-11-17 — Node-local network, RPC and validator-guard bounds enter re-freeze bundle 3 (Claude, Eric 2026-11-17 pre-approval)

- **What:** PR #589 ([ADR-147](docs/adr/ADR-147-node-local-network-bounds.md)): the orphan pool is bounded at 512 blocks and 64 MiB of wire bytes with a per-parent share of 8, and refuses an over-cap block before buffering; the block-sync and RPC rate tables evict their oldest entry at the ceiling; the market HTTP server caps live connections at 256 and the request head at 10 s; a repeat of a counted attestation is ignored before signature checks; the double-sign guard needs `safety.log` itself; the node key file is created owner-only and never overwritten.
- **Why now:** each is a bound on one node's resources or a stricter restart check; none changes a validity rule, a state root or the wire format, so it fits the re-freeze without a re-genesis.
- **Accepted trade-off:** eviction at a full table lets the oldest entry regain its quota after a table's worth of newer ids; the previous behaviour refused newcomers (sync) or grew past the ceiling (RPC).
- **Accepted trade-off (follow-up review):** the market HTTP cap is global with no per-source share, so one client can hold all 256 permits; orphans still never expire, so junk orphans can hold a parent's share until backfill catches the node up. Both are node-local; ADR-147 Consequences records them.
- **Left open:** scoping the safety guard to the current epoch, Windows ACLs for the key file (ADR-049), a per-IP market connection share, the orphan TTL sweep, and the deferred items listed in PR #589.
- **Second review (2026-11-18, Eric 2026-11-17 pre-approval):** an orphan's `tx_root` is now checked before buffering (with the size check, in `insert_block`'s order), so the id-only duplicate skip cannot let a copy with another body displace the genuine block; when the pool is full, a block a buffered orphan waits on displaces the newest orphan nothing waits on (victims are counted first, so a block that cannot be made to fit evicts nothing), so an ancestor walk over a gap larger than the pool lands a run of blocks instead of one; `check_size` returns the wire length so the block is not encoded twice; the RPC table frees `MAX_TRACKED / 16` places per eviction pass instead of one; the node key's directory entry is synced on Unix. Still open: the orphan TTL (a pool held by junk still costs about k^2/2 fetches to catch up k blocks over gossip) and both orphan bounds following a scheduled block byte cap if one lands (#575 does not make `check_size` height-dependent, checked 2026-11-18).
- Review of #589 (2026-11-22): a quarter of the global block-sync serving budget (256 requests, 64 MiB per 10 s) is kept for peers this node dialed, so fresh inbound identities cannot starve its bootnodes and chosen peers. Recorded in ADR-147.
## 2026-11-18 — Signer passphrase files, keystores and VRF encodings hardened (Claude, Eric 2026-11-17 pre-approval)

Eric 2026-11-17 pre-approval put PR #596 into re-freeze bundle 3.

- **Passphrase files are refused when group or others can read them (Unix), as a hard error, not a warning.** Every reader uses `sealed::read_passphrase_file`, now including the `maya2c-signer` CLI. Windows ACLs are not inspected yet. ADR-049 amendment.
- **Remote-signer keystores:** 12-byte passphrase floor on create (the existing sealed-key floor), no overwrite, 0600 on Unix; 64 KiB and KDF bounds on open. ADR-049 amendment.
- **VRF keys and gamma are accepted only in canonical encoding** (spec CRY-5, ADR-024 amendment). It changes which bytes verify, not which values honest code produces.
- **The oracle registry refuses an unusable VRF key when it is installed** (genesis or a signed rotation), not only when a proof under it is verified (CRY-5, ADR-024 amendment, Eric 2026-11-17 pre-approval). It changes which rotations are valid; no value-bearing chain has an oracle registry, so no stored state is affected.
- **The release smoke test writes its passphrase file under `umask 077`**, and the runbooks say passphrase files on Unix must be 0600 on a filesystem that keeps permissions.
- **Not decided here:** replay protection and downgrade policy in the crypto-agility migration engine (an `Account` layout change), and an operator-confirmed floor on slashing-database import. Both stay open.
## 2026-11-21 — DAG-BFT ordering cutoff from the anchor sequence, and node-local BFT bounds (Claude, under Eric's 2026-11-17 pre-approval)

An independent AI review (2026-11-19) replayed two honest engines that batched
commits differently and derived different blocks for one anchor (57 against 61
certificates). The ordering cutoff is now `previous committed anchor round −
GC_DEPTH`, common to every honest node and equal to what a node committing one
anchor per step already used, so only the batched case changes, to match. In
the same change: the commit rule runs only after an insertion or a resume;
proposals and certificates outside the round window or with more parents than
members are dropped before any signature check; fetch replies share one budget;
the safety log records each own message once and opens in bounded memory; the
anchored bootstrap ends at the trusted block and a snapshot import is capped at
64 GiB; rejoin recency uses the epoch that signed the trusted block. Every
finding was checked against the code first. The ones that need an owner
decision (equivocation punishment, weight cap, minimum active set, unconfirmed
time-old tip) are listed in ADR-150, not changed. ADR-150, ADR-060 amendment.

Follow-up from review of that PR (2026-11-22): a follower that resumed at round
`S` builds again only when the previous committed anchor `P` satisfies
`P − GC_DEPTH > S`, not merely when the anchor being built is past
`S + GC_DEPTH + 2`. With skipped anchors the old window opened while the
cutoff still reached below `S`, where the follower holds no history. Without
skips both gates open at the same anchor. ADR-150 item 1, spec/05.
## 2026-11-22 — Mempool keeps only entries that can execute; signature-path pins and verify order (Claude, re-freeze, independent AI audit CT-01, CT-04, CT-06, CT-14, CT-16)

- **No nonce gaps in the pool (CT-01):** a nonce is admitted only when every nonce from the account's up to it is already pooled. After a BFT round, the entries it ordered but did not execute are re-checked once (`Mempool::settle_round`: the included entries leave the pool, then `prune_invalid`; the driver reports them after the block committed, so the order does not change the result) and dropped if they can never execute. The re-check reads committed state only (fee, nonce, gap, balance, vault, breaker; for a share its epoch, budget and spent set) and never re-verifies a signature or re-hashes share work, both fixed at admission: it runs on the BFT consensus task each round, and re-running them over a full pool of v8 frames or over-cap shares would stall rounds (review of #653). Kept: a v7/v8 entry whose suite is sunset while pooled is not pruned here; the executor drops it until it is evicted. Before, such an entry was proposed, fee-free, every round. The node's gossip and RPC pools are linked (`Mempool::link_sibling`), so a nonce pooled in either one is no gap: a wallet that sends nonce n and n + 1 through two RPC endpoints is not refused (review of #653). Cost: out-of-order gossip arrivals are refused at this node until the gap fills, and gossip does not redeliver them; the node they were sent to still holds them. Not covered: an entry that passes every state check but fails on its payload when staged is still re-proposed each round; a strike counter would also evict entries left out for block room, so it waits for the builder to tell the two apart (BACKLOG.md, P2).
- **One share, one pool entry (CT-04):** a mining share is pooled once per `(epoch, work nonce)` per miner; the same work under other transaction nonces is refused. Shares now count toward the 64-per-sender cap and get the same gap rule as transfers, since the executor runs them at exact nonces.
- **`fips204` pinned `=0.4.6` (CT-06):** like the other signature-path crates. `verify_differential_tests.rs` pins that `fips204` (node) and `ml-dsa` (suite 0x30) give the same verdict on corrupted signatures, keys and messages. `docs/CRYPTO_WATCH.md` §5-6 record the domain-separation rule (CT-16) and every pinned signature-path crate, including the `slh-dsa` release candidate.
- **Suite 0x30 verify stops at the first failing half (CT-14):** lattice first, the node hybrid's order. Acceptance is unchanged (both halves required); every input is public, so the early exit reveals nothing secret.
- Admission and verification cost only: no block validity rule, wire byte or state byte changes, so no spec rule or ADR.
## 2026-11-22 — Node-local P2P and RPC bounds from the AI audit (Claude, under Eric's 2026-11-17 pre-approval)

- Recorded in [ADR-151](docs/adr/ADR-151-node-local-p2p-rpc-bounds.md). Each item was checked against the code first; the source is an AI review (2026-11-19), not an external audit.
- No validity rule, state root, gas figure or consensus message changes; no conformance vector changes.
- Bootnodes at public or DNS addresses must pin a peer id, or the node is started with `--allow-unpinned-bootnodes`. The k8s manifest and the operator pass the flag (their pod keys are generated at first start). The public join script already pins.
- Explicit gossipsub peers on every transport (audit P2P-01) are left as they are; changing them changes how validators reach each other, so that is a separate draft for Eric.
- The block-sync byte budgets, the RPC refund keying fix and the market server's remaining limits go on #589's branch (ADR-147), because they sit on the lines #589 changes.
- Review of #655 (2026-11-22), each point checked against the code: a `--catch-up-from` source whose tip is below this node's now abstains instead of vetoing a resume (a lagging restarted peer had sent every node listing it into follower mode); gossip block de-duplication is keyed on the message bytes, not the block id, which does not commit to multisig approvals; attested catch-up crosses a gap over its 1 GiB budget in steps tied to the checkpoint by headers instead of refusing it; the supply walk reads a snapshot taken with the tip and never falls back to walking under the chain lock; `threat_indicators` seeks to its page instead of decoding every indicator per page. ADR-151 items 1, 8, 9, 10 and 15 amended.
- Second review of #655 (2026-11-22), each point confirmed against the code: under `--trusted-block`, ordinary catch-up now compares the trusted id with the linked header at that height before returning any prefix (a stepped round could otherwise import blocks below the trusted height from a chain that contradicts it); the docker compose peers pass `--allow-unpinned-bootnodes` and the VM and Windows join installers require `/p2p/<id>`, with a test over every shipped deploy path; the supply cache is served only on an exact (height, tip) match, since a heaviest-work reorg can lower the height. ADR-151 items 4, 8 and 10 amended.

## 2026-11-22 — Re-freeze integration, genesis candidate: merge order and conflict resolutions (Claude, under Eric's 2026-11-17 pre-approval)

Branch `refreeze/integration` (draft #626) re-merged onto master `d6a63a5d` to show the whole re-freeze bundle composes; it is a check, not a merge path.
- **Order:** master → #540 → #555 → #500 → #549 → #641 → #643 → #544 → #542 → #577 → #584 → #589 → #596 → #650 → #653 → #655. #506, #522, #575, #570, #573 and #598 were already in at their current heads; #621 is on master.
- **Docs and tables:** every conflict in README, ADR-082, ADR-142, the ADR index, `spec/README.md`, `spec/05-consensus.md`, `spec/04-fees.md`, invariant 34, `features.toml`, `architecture-vision.md`, `CATALOG.md`, `workspace-map.md`, `CRYPTO_WATCH.md`, `THREAT_MODEL.md` and the regenesis checklist kept both sides: rows unioned, test lists unioned, the higher section version kept where both bumped. ADR-142 keeps amendments 5 (#643), 6 and 7 (#549) in number order. Two stale numbers corrected on the way: #544's CATALOG row named a 1,172,800-block council sunset lead, which #500 made 3,592,000 (`MIN_MIGRATION_WINDOW` = 2,592,000); #577's wasmtime 48.0.8 pin gave way to #641's 48.0.10, keeping #577's manifest-and-lock pin test.
- **Code, textual:** `Overlay::root_plan` (#540 and #643 placed it differently; kept once); `StateDB` keeps both #643's `contracts_root` and #653's `share_proofs`; `scan_range` (#555) and `scan_prefix_page` (#655) are both kept; #653's `check_against_state` replaces the inline mempool checks, with #570's work-claim refusal kept in front of it; #653's `share_payable` keeps #570's `WorkKind` gate; #577's config test sits beside #549's ISA tests.
- **Code, semantic (compiled red, then fixed):** #655's snapshot `StateView` read the single staking record that #555 replaced with per-record storage (E0599), so the view now sums validator and payout records with the same helper as `StateDB::staking_held`, and the staking differential test asserts the two agree. Tests added by #584 and #653 built `MiningShare` without #570's work-kind byte (E0063). #540's root-cache test named `maya_fee_market` directly, which `the_fee_market_is_reached_only_through_its_genesis_gate` refuses; it reads the cap through `state::fees`.
- **Gas golden vectors (#641 × #549):** #641 recorded its seven vectors under master's schedule; #549 changes the schedule on purpose (native work charged in fuel), so all seven moved (e.g. `empty` 2 → 210,302). Re-recorded on the integration branch with wasmtime pinned to 48.0.8 and to 48.0.10: identical costs, boundaries and outputs on both, so the 48.0.10 bump still moves nothing.
- **Pre-existing, not caused by the bundle:** `byzantine_guard_tests::slow_honest_peers_carrying_valid_traffic_are_never_quarantined` fails on this Windows host on master `d6a63a5d` itself, and at two intermediate points of this branch; left as it is and reported.

### 2026-11-22 — Staking dust floor and operator-entry reserve (decision brief 2, review R-7), pending Eric

Built as a draft `[needs-eric]` PR stacked on #555; nothing here is decided until Eric chooses.

- **Proposed (brief 2 option A):** mainnet `min_delegation` = 100 coins (10^26 base units), up from
  the 1 coin decided 2026-10-30. Closing one validator to new delegators then locks 1,638,400 coins
  (16,384 positions), closing the chain 13,107,200 (131,072), against 16,384 and 131,072 at 1 coin.
  Era 1 issues about 2,625,000 coins a year at one block a second. Cost of the choice: a holder under
  100 coins needs a pool. One line in `xtask/src/genesis_check.rs`; no record format change.
- **Proposed (R-7):** an operator unbond that would open a new unbonding entry is refused once
  positions reach the ceiling plus `OPERATOR_ENTRY_RESERVE` = 16,384 (spec STK-12). Positions are then
  bounded whatever the number of validators. A full operator exit of at least `min_self_bond` is
  exempt, so an operator can always leave (a slashed one tops up to the minimum first); each exempt
  entry holds at least `min_self_bond`, so capital bounds them (positions at most 167,143 at the
  1,000-coin bond with every coin issued bonded). Filling the reserve takes 2,341 validators at
  1,000 coins, kept refilled at every boundary; while it is kept up, operators' partial unbonds are
  refused, not merely delayed. An earlier draft refused full exits too; review caught it.
- **Not done, for Eric:** a registration ceiling (needs a validator count in the meta record, a
  layout change); option B (evict the smallest position at a full ceiling), suggested after genesis.
- **Audit claims checked:** brief 2's ceilings (131,072 and 16,384) and the 1-coin floor are right;
  R-7's "operator Unbond skips the ceiling" and "registration is uncapped" are right. The brief's
  "total supply MISSING" is filled from `deploy/genesis/mainnet/README.md` (19,687,500 coins over the
  halving eras).

