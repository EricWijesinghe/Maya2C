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

## 2026-10-05 — Launch without outside people (ADR-042, Eric)

Eric chose all three recommended options:
- **Gate 5:** a foundation validator set, four project-run validators on four
  machines (PC + Oracle Always Free), stated publicly as foundation-operated.
- **Gate 6:** a self-run daily attacknet for four weeks plus a credit-only
  break-it programme.
- **Gate 7:** a pre-audit package, free audit programmes applied for, and
  mainnet labelled "external audit pending".

Why: investor money arrives only after three months of mainnet, so every
gate waiting on people or money would have blocked the thing that pays for
them. Each gap is written on the site, never relabelled away.

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
inflate. It is also the simplest model to explain to a first investor.

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

## 2026-10-28 — Supply, capacity and the governance deposit (Claude decided, Eric delegated)

- **Supply stays 21M coins at 18 decimals (2026-10-26 stands).** Capacity is already the
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
