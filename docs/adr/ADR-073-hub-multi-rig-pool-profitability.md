# ADR-073: Hub multi-rig, pool view and profitability estimate

**Status:** Proposed (2026-11-01). Ledger item C1. Nothing here is built;
the Hub stays `planned` in `features.toml` until a phase below lands with
its test output in `reports/`.
**Date:** 2026-11-01

## Context

ADR-045 gave the Hub one miner. The Hub today runs exactly one
`maya2c-gpu-miner` child, for one payout key, on one device:

- `apps/hub/src-tauri/src/miner.rs:46-68` `MinerState` holds one run's numbers.
- `apps/hub/src-tauri/src/miner.rs:143-184` `Miner::start` spawns one child
  with `--mine --json --rpc <url> --threads N --key-file <file> [--gpu]`.
  There is no device argument, so `--gpu` means the miner's default device
  (`bins/maya2c-gpu-miner/src/main.rs:34-36`, `--device` default 0).
- `apps/hub/src-tauri/src/miner.rs:213-224` `Miner::state` notices that the
  child exited and sets `status = "miner exited (...)"`. Nothing restarts it.
- `apps/hub/src-tauri/src/gpu.rs:12` reads temperature, power, limit, load,
  memory and clock. Nothing acts on them. There is no thermal limit.
- `apps/hub/src-tauri/src/lib.rs:84-89` `miner_start` takes a binary path from
  the UI and runs it. The path is not checked against anything.
- `apps/hub/src-tauri/src/lib.rs:34` `PUBLIC_RPC`, and `apps/hub/src-tauri/src/node.rs:31-38`
  `check_url` accepts any http(s) URL. That is the only pool choice.

What the chain offers, which the profitability estimate must use:

- `bins/maya2c-gpu-miner/src/share.rs:245-254` fetches `get_mining_work` for
  the payout address. The reply is `Work` (`share.rs:92-107`), built by
  `crates/node/src/rpc/server.rs:1125-1157`: `epoch`, `header`, `target`,
  `share_reward`, `shares`, `target_shares`, `epoch_blocks`,
  `max_shares_per_block`.
- Per-share reward is capped by the epoch budget: `share_reward()` is
  `min(budget / target_shares, budget − paid)` (`crates/node/src/state/mining.rs:257-265`).
  The budget is `bps_of(epoch_reward, miner_share_bps)` (`mining.rs:249-251`),
  and `epoch_reward` halves on a schedule (`mining.rs:208-212`).
- A share is a signed transaction whose payload is only `epoch` and `nonce`
  (`crates/node/src/core/mining_payload.rs:13-18`). The payee is the sender.
  Submission is `share.rs:448-470`.
- The share header binds chain tag, epoch, anchor and miner
  (`crates/node/src/state/mining.rs:441-453`).

Two facts constrain the design:

1. **The miner trusts the work it is given.** `share.rs:315-316` decodes
   `work.header` and `work.target` and hashes against them. It never recomputes
   `share_header(chain_tag, epoch, anchor, own_address)`. A node or gateway
   that is not the one the user intends can therefore make the miner burn
   power on work that the chain will refuse. It cannot redirect the reward,
   because the payee is the signed sender. The loss is electricity and time.
2. **The payout key is a plaintext hex seed.** `share.rs:188-201` writes
   `maya2c-miner.key`-style hex to disk and reads it back. The Hub's default
   path is `app_data_dir/miner.key` (`lib.rs:52-58`). ADR-049 sealed validator
   keys; the miner key was never sealed. Every rig that mines for one
   address must hold that seed to sign shares.

Standing Orders that bind this work: visible and opt-in mining only, nothing
persistent without consent, no admin or driver changes (`platform-compliance-directive`
memory, 2026-10-12); measured numbers only, with the source labelled
(Standing Order 2 and 3); no secrets in logs or chat (Standing Order 5).

## Decision

**1. One miner process per GPU, supervised by the Hub.** `Miner` becomes a
map of workers keyed by device index. Each worker is the existing
`maya2c-gpu-miner --mine --json --device N` child and keeps today's event
parser (`miner.rs:90-139`). A CPU worker is also allowed. Workers are
independent: one crash does not stop the others.

**2. Several rigs under one payout address.** Protocol payees are the signed
sender, so there is no way to pay one address from several keys. A rig is
therefore a machine that holds the payout seed. Rules:

- The seed is sealed on each rig with the OS protection API (DPAPI on Windows,
  Keychain on macOS, Secret Service or a passphrase file on Linux). This is the
  "next step" ADR-049 already names. Plaintext `miner.key` is refused in a
  release build.
- The controller Hub never holds the seed. It only holds the rig list and the
  rig's public key.
- Pairing is one-time and visible: the rig shows a six-word fingerprint of its
  public key, the controller shows the same words, and the user confirms.
- Control messages between controller and rig are signed by the controller's
  ML-DSA-65 key (the chain's suite, `crates/signer` / `maya-crypto-pq`, as in
  `apps/chat/src/identity.rs:83`), carry a monotonic nonce and a timestamp,
  and are refused when stale. Transport is HTTPS on the LAN with the rig's
  certificate pinned to the paired key. No cloud relay.

**3. Watchdog.** Pure state machine in `miner.rs`, unit-tested without a
process:

- Restart a worker on exit, with exponential backoff: 2 s, doubling, capped
  at 5 min. After 5 exits within 10 minutes the worker stops and raises an
  alert. A deliberate stop never restarts.
- Stall: the miner emits `hashrate` every 2 s (`share.rs:34`). No event for
  60 s means stall: stop, then restart under the same backoff.
- Thermal: `temperature.gpu` above the ceiling stops that GPU's worker and
  raises an alert. The worker restarts when the reading falls 5 °C below the
  ceiling. The default ceiling is 80 °C and is labelled as a default in the
  UI. Where the driver reports a slowdown threshold, the Hub shows it next to
  the ceiling. It does not invent one. The GPU query gains
  `temperature.gpu.tlimit` or the driver's equivalent, read-only.
- **No power-limit writes.** Changing power limits (`nvidia-smi -pl`) needs
  administrator rights and changes driver state. Mining apps that do this are
  the ones AV flags. A thermal stop is visible, reversible and needs no
  elevation.

**4. Alerts are local.** Alerts appear in the Hub and as the OS notification
the Tauri notification plugin provides. Triggers: worker exited and not
restarted; backoff limit reached; thermal stop; no accepted share for a
configurable window; no balance growth for a configurable window; rig
unreachable for 2 min. No Telegram, e-mail or webhook sender. Those need a
stored bot token or SMTP secret on the rig, which Standing Order 5 forbids.

**5. Pool selection.** The protocol has no pool. The node that serves
`get_mining_work` is the pool, and it cannot take a cut of the reward, so
"pool" here means which node a rig asks for work. Three choices:

| Choice | Source | Default |
|---|---|---|
| Maya2C public node | `lib.rs:34` `PUBLIC_RPC` | yes |
| Own node | user URL on loopback or LAN | no |
| Other node | user URL, not on the allow-list | no, needs confirmation |

- **Allow-list.** The Hub ships a list of known endpoints, signed with the
  release key and delivered through the update channel (#258). A custom
  endpoint is accepted only after the user confirms it, and then shows as
  `unverified`.
- **Verification before custom endpoints.** Before a rig accepts work from an
  unverified endpoint, it recomputes `share_header(chain_tag, epoch, anchor,
  own_address)` (`mining.rs:441`), with `chain_tag` from `get_chain_info` and
  `anchor` from `get_block_by_height` read from a second endpoint. Work whose
  header differs is refused and the endpoint is reported. This closes the gap
  at `share.rs:315`, which is a bug in the miner, not only in the Hub.
- **Stratum is not offered.** `crates/stratum-v2` and `bins/pool-service` exist
  but are not on the share path (ADR-044, "Not covered"). Supporting a stratum
  URL would add a second work protocol with a pool operator who sees every
  share. That is a new ADR, not a field in this one. The request's "custom
  stratum URL" becomes "custom node URL" here; the difference is reported,
  not worked around (Standing Order 9).

**6. Profitability estimate, on-chain inputs only.** Shown as an estimate,
with the formula on screen. Inputs come from `get_mining_work`, `get_chain_info`
and the rig's own measured hash rate. Accepted shares are never an input to
pay.

Definitions:

- `r` = `share_reward` (base units per share, from `Work`).
- `B` = epoch budget. `Work` does not carry it. While the budget is unspent,
  `r = budget / target_shares` (`mining.rs:257-265`), so `B ≈ r × target_shares`.
  The approximation is off only for the one share that the remainder caps.
  The node's `miner_budget` (`mining.rs:249`) stays the authority; the Hub
  test (item 5) checks the approximation against it.
- `H` = hashes per share = `2^256 / T`, where `T` is `target` as a 256-bit
  integer.
- `h` = this rig's measured hash rate (the miner's own count).
- `c` = this rig's expected claims per epoch = `h / H × epoch_seconds`.
- `S` = claims in the epoch so far (`shares`), `b` = blocks into the epoch
  (chain height minus the epoch's first block, see Test plan item 4).
- `N` = projected network claims per epoch = `S × epoch_blocks / b`.
- `P` = epoch miner payout = `min(B, N × r)`.
- Rig's expected payout per epoch = `P × c / (N + c)`.
- Per day = `(86400 / epoch_seconds) × rig payout`. `epoch_seconds =
  epoch_blocks × measured block time`.
- Net per day = per day − `power_W / 1000 × 24 × kWh price` (price typed by
  the user; no default).

Labels on screen: "Estimate. Projects the on-chain epoch budget from claim
counts already on chain. Assumes the network rate holds. Not a payout
promise." The realised figure is shown beside it, from balance growth
(`miner.rs:54-61` `earned`), which is the only pay the Hub reports.

Honesty checks in code: the estimate is refused (shows "not enough data") until
at least one full epoch of claims is observed, and when `b < 1% of
epoch_blocks` because the projection is then unstable.

**7. Mining stays visible and opt-in.** No autostart. Mining starts only on a
user Start for each worker. Closing the Hub stops every worker (today's
behaviour, `lib.rs:91-94` `miner_stop`; kept). A tray indicator shows mining is
running. No service, scheduled task or registry key is installed.

## Alternatives considered

- **Per-rig subaddresses under one payout.** Would avoid copying the seed. The
  payee is the signed sender (`mining_payload.rs:13-18`), so this needs a
  consensus change: a payee field in `MiningShare`, plus the header and
  conformance vectors. Rejected for this ADR as too large a change for a Hub
  feature. Revisit with its own consensus ADR if seed copies become the
  limiting risk.
- **A stratum adapter.** Rejected, see decision 5.
- **A cloud dashboard that aggregates rigs.** Rejected. It would send payout
  addresses and telemetry to a server nobody has audited, and it is the only
  place a fleet-wide compromise could start. LAN controller only.
- **Power limits through `nvidia-smi -pl`.** Rejected: needs elevation and
  changes driver state. Thermal stop is the substitute.
- **Profitability from external network-hash sites.** Rejected: not on-chain,
  not reproducible, and the request asked for on-chain data only.
- **Alerts through a messenger bot.** Rejected: stores a secret on the rig and
  sends telemetry out of the machine.
- **Keep one miner and use the GPU's own multi-device support.** Rejected: the
  miner takes one `--device` (`main.rs:34-36`), and a per-process supervisor
  gives isolated crashes for free.

## Consequences

- Each rig holds a copy of the payout seed. That is the largest new risk. It
  is bounded by sealing and by the pairing ceremony, and it is still worse
  than one key on one machine. Users must be told.
- The LAN control channel is a new network surface. It is listening code,
  so it needs a fuzz target and a review (see Test plan).
- The miner change (header recomputation) touches the miner binary, not only
  the Hub. It must ship with a miner release, and an old miner keeps working
  against trusted endpoints.
- Thermal stops cost hash rate. That is the point; the Hub shows the cost
  per day in the estimate.
- The estimate is a projection. It will be wrong when the network changes
  quickly. The realised line is there so the difference is visible.

## Security model

| Asset | Threat | Control |
|---|---|---|
| Payout seed on each rig | theft by malware or a copied file | sealed at rest (DPAPI / Keychain / Secret Service); plaintext refused in release; no seed on controller |
| Control channel | rogue controller or LAN attacker issues commands | ML-DSA-65 signed commands; nonce and timestamp; pinned rig key; LAN only; no default listen on 0.0.0.0 |
| Work from an endpoint | wasted power on invalid work | header recomputed against two endpoints before hashing (decision 5) |
| Payout reward | redirected by an endpoint | not possible: payee is the signed sender; header binds own address |
| Miner binary | a swapped `maya2c-gpu-miner` | Hub accepts only the release-signed miner (path inside install dir, hash in signed manifest); arbitrary path needs the user to confirm and is shown as unsigned |
| UI / webview | script injection | existing CSP (`tauri.conf.json:23`) unchanged; no new remote origins |
| Privacy | payout address seen by the endpoint | disclosed in the endpoint confirmation; default endpoint is ours |
| Compliance | flagged as a miner by AV | no service, no driver writes, no admin, no hidden window, signed binaries (platform-compliance-directive) |

## Test plan

Each item is a test that fails when the feature is broken. Nothing here has
been run; the commands are for the implementer.

1. **Watchdog state machine (pure, unit).** Table of exit, stall, and backoff
   sequences: restart times are 2, 4, 8 ... capped at 300 s; the stop after
   five exits in ten minutes; a deliberate stop never restarts.
2. **Thermal hysteresis (pure, unit).** Reading sequence 79, 81, 79, 75, 74
   at ceiling 80 stops at 81 and restarts at 74 (5 °C below). A missing
   reading never stops or restarts.
3. **Per-device workers (integration).** Two `--device` children on the
   devnet from `xtask/src/mining_localnet.rs`; kill one; the other keeps
   emitting `hashrate`.
4. **Epoch position (conformance).** `b` must equal the blocks since the
   epoch's first block for the node's epoch rule. Check against
   `crates/node` epoch arithmetic with a vector at each boundary block.
5. **Estimate formula (pure, golden vectors).** Fixed inputs `(r, target_shares,
   target, h, S, b, epoch_blocks, power, price)` and expected outputs to the
   base unit. Include: budget unspent; budget spent (`P = B`); `b` too small
   (must refuse); zero claims (must refuse, not divide by zero).
6. **Header recomputation (conformance).** The Hub/miner recomputed header
   must equal `share_header` for the node's own vector set. A mismatched
   endpoint must be refused before any hashing. A test with a lying endpoint
   proves no hash is computed on the refused work.
7. **Allow-list (unit).** Unsigned or bad-signature allow-list refused; custom
   URL without confirmation refused; `check_url` keeps rejecting `file:`.
8. **Control channel (unit + fuzz).** Replayed command refused; stale timestamp
   refused; wrong key refused; fuzz target on the frame parser with a
   corpus in `fuzz/`.
9. **Seed handling (unit).** A plaintext key file in a release-flagged build
   is refused; sealed round trip; wrong OS protection context fails closed.
10. **Estimate vs realised (devnet soak).** Run one rig for ten epochs on the
    devnet; record the estimate at each epoch start and the balance growth at
    epoch end; report both with the command output. No figure is written into
    docs until it is measured.

## Rollout

- **Phase 0 (this ADR).** Accepted after review by the security reviewer and
  the rust reviewer, as Workflow Rule 2 asks for mining payout code.
- **Phase 1, one rig, no protocol change.** Watchdog, thermal stop, alerts,
  per-device workers, estimate with golden tests. Hub only. Standing Order 3:
  the estimate says it is an estimate in the UI and in the output.
- **Phase 2, miner header recomputation** (`share.rs:315`), released as a
  miner version. Lands before any custom endpoint is offered.
- **Phase 3, endpoint allow-list and custom endpoints** in the Hub, with
  confirmation and the `unverified` label.
- **Phase 4, sealed seed and rig pairing,** multi-rig controller. Blocked on
  ADR-049's DPAPI step being implemented for the miner key.
- Each phase: `features.toml` entry in the same change; site and Universe
  status updated when the Hub's status changes (feedback memory,
  2026-10-29). No phase is announced before its test output is in `reports/`.

## Revisit when

- Consensus gains a payee field (per-rig subaddresses become cheap).
- The node exposes epoch start height directly, which removes item 4's
  arithmetic.
- A stratum or pool protocol is requested by miners who cannot use the
  node's RPC. That is a new ADR with its own trust model.
- Measured estimate error over ten epochs exceeds the bound the team sets in
  the ADR-073 soak report.
