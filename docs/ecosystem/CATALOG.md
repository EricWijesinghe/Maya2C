# Ecosystem catalog

Every app and service planned for the Maya2C ecosystem in one list, to be
built, debugged and shipped **one at a time**. Started 2026-11-10 at Eric's
request: "populate the ecosystem so we can build and drop one by one later".

This is a **staging list, not a status claim.** The status of anything
built is in `features.toml` (`cargo xtask coverage`). An entry here moves
through the usual gate before code (CLAUDE.md, Workflow 5):

1. **Answer the three questions** from [ECOSYSTEM.md](../ECOSYSTEM.md): who is
   it for, what does it do better than what they use today, and how is
   success measured.
2. Give it a status tag in [architecture-vision.md](../architecture-vision.md)
   and a `planned` entry in `features.toml`.
3. Write an ADR if it is a design choice. If it touches consensus, it needs
   an activation height and the invariant review (ADR-002, ADR-016).
4. Code, tests, a report in `reports/`, then `features.toml` to `working`.

Rules every entry inherits:
- Apps are **separate programs** sharing identity, the SDK and the design
  system. They never depend on the node crate (ECOSYSTEM.md section 1).
- No app holds another app's keys (ADR-045, ADR-083).
- No "first", "only" or "unprecedented" without a dated
  `docs/prior-art/` search (Standing Order 10).
- Anything that writes new state is consensus work: it is gated behind an
  activation height, never a cargo feature (ADR-002).

**Columns.** *Builds on* is code that exists in this tree today. *Gate* is
what must happen before code: an ADR, consensus review, an approval or an
outside dependency. *Size*: S is up to a few days, M is one to three weeks,
L is more. *Wave* is when, defined at the end of the file.

## A. Core user apps

| id | What, for whom | Builds on | Gate | Size | Wave |
|---|---|---|---|---|---|
| hub-launch | Open Wallet and Chat from the Hub through an OS link scheme (ADR-083 decision 4). **Built on `feat/hub-launch`; installed-build registration not yet run** | `apps/hub`, `apps/wallet-gui`, `apps/chat-gui` | installed NSIS/MSI and `.desktop` check (`docs/app-link-schemes.md`) | S | W0 |
| address-text-everywhere | Bech32m `maya1…` in the wallet GUI, explorer, pool, chat and docs (ADR-081 leftovers) | `crates/address` | none | S | W0 |
| signed-installers | Signed installers for Windows, Linux and Android for every app, with auto-update | `apps/app-update`, ADR-080 | code-signing cost (Eric) | M | W0 |
| wallet-v2 | Clear signing, smart-account recovery without a paper seed, address book, payment requests | `apps/wallet-gui/core`, `crates/smart-account`, `crates/clear-sign` | gate: [ADR-085](../adr/ADR-085-chat-v2-groups-on-mls.md)/[ADR-086](../adr/ADR-086-wallet-v2-seedless-recovery-and-clear-signing.md) Proposed, decisions for Eric listed | L | W1 |
| wallet-mobile | Android wallet (light client); iOS after the MacBook | `crates/light-client`, wallet core | platform targets (memory 2026-10-12) | L | W2 |
| wallet-extension | Browser extension wallet for web apps | wallet core, `crates/web-security` | ADR (dApp connection protocol) | M | W2 |
| hardware-wallet | Ledger app for ML-DSA signing | `apps/ledger-maya2c` | device memory for PQ signatures, Ledger review | L | W3 |
| chat-v2 | Group chats, attachments, multi-device | `apps/chat`, ADR-074 | gate: [ADR-085](../adr/ADR-085-chat-v2-groups-on-mls.md)/[ADR-086](../adr/ADR-086-wallet-v2-seedless-recovery-and-clear-signing.md) Proposed, decisions for Eric listed | L | W1 |
| chat-pay | Send MAYA inside a chat; the wallet signs (ECOSYSTEM.md section 3) | chat + wallet hand-off | hand-off protocol ADR | M | W1 |
| explorer-v2 | Graphs, address pages in Bech32m, validator and mining pages, screenshot-diff CI (C10). Built on `feat/explorer-v2`; uptime not shown (RPC has no uptime); explorer baselines pending | `apps/explorer` | none | M | W1 |
| status-v2 | Public network status with a history of incidents and gate progress. Started 2026-11-10: the site's [Live network](https://maya2c.dev/network/) page draws validators, blocks and finality from the existing endpoints | `apps/status`, `apps/sentinel` | none | S | W1 |
| notifications | Opt-in alerts (payment received, validator down) without a central server | `apps/hub` `notify.rs`, sentinel ntfy | privacy statement | M | W2 |

## B. Money and commerce

| id | What, for whom | Builds on | Gate | Size | Wave |
|---|---|---|---|---|---|
| merchant-checkout | Payment links, QR invoices and a webhook for shops | wallet core `payment.rs`, SDKs | none | M | W1 |
| pos-terminal | Point of sale on an Android phone: QR request, wait for finality, receipt | merchant-checkout, light client | none | M | W2 |
| recurring-payments | Subscriptions and payroll with spending limits | `crates/smart-account` session keys; protocol execution at a height: ADR-109 (proposed, `sp:` prefix) | consensus review (ADR-109) | M | W2 |
| escrow | Two-party escrow and milestone payments | `crates/htlc-lattice`, `bins/htlc-watcher` | none | M | W2 |
| flash-payments | Instant off-chain payment channels for small, frequent payments | `crates/l2-flash` | ADR (watchtower model) | L | W2 |
| exchange-ui | Swap and order-book GUI; must pass the B38 ship gate (`min_out` from a quote, 1% default) | `crates/dex`, `compose_swap` | B38 gate, slippage tests | M | W2 |
| treasury-multisig | Multisig and MPC treasury app for teams and DAOs | `crates/custody-mpc`, `crates/treasury` | invariants 18 and 19 review | L | W2 |
| inheritance | Time-locked recovery to heirs if an owner stops signing | smart-account guardians | legal wording review | M | W3 |
| btc-bridge-app | BTC in and out with SPV proofs | `crates/btc-bridge`, `crates/btc-spv`, `crates/deposit-watcher` | security review, bridge ADR | L | W3 |
| rwa-issuance | Tokenized real-world assets with issuer rules | `crates/rwa`, `crates/permissioned-finance`, `crates/iso20022` | compliance and legal (Eric) | L | W3 |
| grants-quadratic | Community grants and crowdfunding with quadratic matching | `crates/governance`, `crates/personhood` | Sybil-resistance review | M | W3 |
| shielded-pool-enable | Turn on the shielded pool (private transfers), closing the privacy gap to Monero and Zcash ([COMPETITORS.md](COMPETITORS.md) section 4) | `crates/node/tests/shielded_tests.rs`, ADR-037 | independent audit of the join-split circuit, then an activation height (ADR-002) | L | W3 |

## C. Identity and trust

| id | What, for whom | Builds on | Gate | Size | Wave |
|---|---|---|---|---|---|
| identity-app | One `did:maya2c` across every app; verifiable credentials wallet | `crates/identity` | ADR (credential formats) | L | W2 |
| personhood | Human verification without biometrics stored on chain | `crates/personhood` | privacy statement and red team | L | W3 |
| names | Human-readable names for addresses | new state prefix `nm:`: ADR-105 (proposed) | consensus review (ADR-105) | M | W2 |
| notary | Timestamp and prove that a document existed (hash only, never the file) | transaction memo or VM contract | none if hash-only | S | W1 |
| reputation | Portable reputation for marketplaces and freelancers | identity-app | ADR (anti-gaming) | L | W3 |

## D. Builders and operators

| id | What, for whom | Builds on | Gate | Size | Wave |
|---|---|---|---|---|---|
| sdks-1.0 | Stable Rust, TypeScript, Python and Go SDKs with versioned releases. **Status:** Rust gateway client `maya2c-sdk` and Go Bech32m address codec built; not published (features.toml `rust-gateway-sdk`) | `sdks/`, `bins/l1-wallet` | publishing approval (Eric) | M | W1 |
| dev-portal | Docs, tutorials, playground and contract templates | `apps/dev-hub`, `apps/portal`, `docs/ecosystem/COURSE.md` | none | M | W1 |
| public-api | Indexer API (REST, later GraphQL) for wallets and dashboards | explorer indexer, `crates/api-gateway` | per-IP rate limits, hosting cost | M | W1 |
| mcp-server | AI assistants can read the chain and draft (never sign) transactions | `bins/maya2c-mcp` | key-safety review (pending, Eric); built per ADR-084 | S | W1 |
| node-in-a-box | One-click validator and node installer with health checks | one-line-node-join, `apps/status` | none | M | W1 |
| validator-console | Validator dashboard: uptime, rewards, slashing risk, alerts | `apps/sentinel`, staking RPC | none | M | W2 |
| pool-dashboard | Pool operators' and miners' web view | `bins/pool-service` dashboard | B34 to B37 pool items | M | W2 |
| contract-studio | Write, test and formally check WASM contracts | `crates/vm`, `crates/contract-safety`, `crates/multivm` | none | L | W3 |
| testnet-kit | Faucet, local devnet and attack lab for developers | `apps/faucet`, `cargo xtask localnet`, attacknet | none | S | W1 |
| finality-latency-bench | Per-transaction submit-to-finalized latency, with the mix, hardware and command stated; closes the measured-finality gap ([COMPETITORS.md](COMPETITORS.md) section 4) | `cargo xtask bench-finality` (localhost, one machine, measured 2026-11-11; stagenet NOT RUN), `cargo xtask bench-tps`, `reports/bench/` | none (non-consensus) | S | W0 |
| validator-recruitment | Public validator onboarding and operator guide, so the validator count can grow beyond the disclosed launch set | `apps/status`, `apps/sentinel`, one-line-node-join | owner decision: no third-party validators before mainnet (2026-10-28 directive) | M | W1 |
| independent-audit-program | Scoped external audit of the consensus, crypto and ledger crates, with a public report | `features.toml` `kani-model-checking`, `crates/build-guard` | Eric approval and budget | L | W1 |
| succinct-state-proofs | Phone-sized proofs that a Maya2C state or epoch hand-off is final. Phase 0 (M0) is built as a native finality light client in `crates/light-client` (ADR-089): quorum checkpoints and committee handoffs, no proof system. Later phases: phone benchmarks, an off-chain epoch-chain proof, then a committee commitment that needs consensus review | `crates/light-client/src/finality.rs`, `crates/zk-stark`, research note `docs/research/succinct-light-client-2026-11-10.md` | phase 0 built; phases 1-2 none; phase 3 consensus ADR and activation height | M | W3 |

Status of the two rows above: **testnet-kit** and **node-in-a-box** are
stubs (tool code in `xtask/src/kit.rs` and `xtask/src/node_box/`, unit-tested,
ledger entries `stub`). Guides: [testnet-kit.md](testnet-kit.md),
[node-in-a-box-windows.md](node-in-a-box-windows.md),
[node-in-a-box-linux.md](node-in-a-box-linux.md). Not yet run end to end.

## E. AI-native network (post-mainnet; models never in consensus)

| id | What, for whom | Builds on | Gate | Size | Wave |
|---|---|---|---|---|---|
| verified-inference | Miners' GPUs run open-weight models and are paid per verified job | ADR-044 WorkClaim, ADR-079 | ADR-079 phases, activation height | L | W3 |
| confidential-ai | Inference on private data with attestation | `crates/confidential-ai` | TEE invariant must be numbered first (CLAUDE.md) | L | W4 |
| agent-wallets | AI agents with spending limits and human approval for large payments | smart-account sessions, mcp-server | ADR (limits and kill switch) | M | W3 |
| ai-marketplace | Market for models, data sets and jobs | verified-inference | legal (data licences) | L | W4 |
| sentinel-ai | Watchdog AI that proposes and never acts (ADR-077) | `apps/sentinel` | ADR-077 phases; P1 detectors built (ADR-101), P2-P5 wait for Eric (local vs API model, quorum sentinel hosts) | L | W3 |
| on-device-assistant | Wallet and chat helpers that run locally; nothing leaves the device by default | ECOSYSTEM.md section 4 | data-leaves-device statement | M | W3 |

## F. Fields (detached apps on the shared identity)

| id | What, for whom | Builds on | Gate | Size | Wave |
|---|---|---|---|---|---|
| defsec-intel | Shared threat intelligence that defenders can verify | `crates/threat-intel`, `bins/threat-firewall` | none | M | W2 |
| offsec-lab | Authorized-scope testing lab, sandboxed, own-assets only | offsec-sandbox (ADR-063) | scope and abuse policy | M | W3 |
| mesh-network | Messaging and payments over mesh and radio where the internet is poor | `hal/radio-transport`, `crates/mesh-api` | spectrum rules per country | L | W3 |
| iot-anchor | Device data anchored with proofs (sensors, supply chain) | `hal/iot-anchor`, `hal/iot-firmware` | none | M | W3 |
| genetics-vault | Owner-controlled genomic data: consent and an access log, data off chain | identity-app | data-sovereignty only (memory), legal | L | W4 |
| research-records | Timestamped, citable research and open data sets | notary, `crates/da` | none | M | W3 |
| render-market | GPU rendering and simulation jobs with checkable results | verified-inference | ADR-079 | L | W4 |
| civil-defence | Disaster coordination and resilient communications, civil uses only | mesh-network | use policy | L | W4 |
| quantum-harbor | Move assets from quantum-vulnerable chains under PQ protection (flagship, memory 2026-10-05) | `crates/quantum-harbor` | bridge and security review | L | W3 |

Research subsystems (space links, bio, photonic and others) are already
`planned` in `features.toml` with their own gates, and are not repeated here.

## G. Future-proofing already fixed at genesis

These are what make the chain hard to outdate. Each is in the frozen tree
with a test, so it is a fact to check, not a promise. Lead claims against
named chains are only those in
[prior-art/pq-l1-comparison-2026-11.md](../prior-art/pq-l1-comparison-2026-11.md).

| Property | Where it is pinned |
|---|---|
| Swappable cryptography: a suite registry and a sunset record, so a broken algorithm is retired by schedule, not by a hard fork | invariants 29 and 30, ADR-082 |
| Two post-quantum signatures on every transaction (ML-DSA plus SLH-DSA, two different hardness assumptions) | `Transaction::verify`, invariant 31 |
| Post-quantum transport and ML-DSA-87 validator keys | Noise prologue (#377), DECISIONS 2026-10-28 |
| Upgrades on chain under compiled leads; no feature switches on without a scheduled height | ADR-082, invariant 34 |
| 256-bit amounts at 24 decimals: no ceiling to outgrow | ADR-046, ADR-068 |
| Every state prefix under the state root; an uncovered key is caught | invariant 25, `uncovered_keys` (#395) |
| Network-prefixed, checksummed addresses | ADR-081 |
| A security council bounded by compiled rules: it can pause one module for at most `max_pause_blocks` (the pause expires by itself), or schedule a suite sunset no sooner than `COUNCIL_SUNSET_LEAD_BLOCKS` (1,172,800 blocks, about 13.6 days); it cannot move funds, mint, or schedule a version. It has no term limit | `crates/node/src/state/council.rs`, `CouncilGenesis` in `crates/node/src/genesis.rs`, invariant 34 (ADR-082) |

**Gaps, stated plainly.** These are the places where another chain is ahead
today:
- **Decentralization:** the launch is a disclosed single machine, which leaves
  validator count and geography as gaps.
- **Audit:** there is no independent audit yet; the self pre-audit is not one.
- **Track record:** the chain has no time in production yet.
- **Ecosystem size:** there are no third-party apps yet.

Waves W1 to W3 exist to close them. The per-chain comparison, with sources
and each trailing cell mapped to a row here, is in
[COMPETITORS.md](COMPETITORS.md).

## H. The long journey (vision; Eric 2026-11-10)

The chain is the first step toward an operating system and an ecosystem
for a civilization far larger than ours. Eric's framing is "type VII". These
are directions, not plans: none starts before W4, and each needs research
and an ADR first.

| id | Direction | Builds on | Gate |
|---|---|---|---|
| maya-os | A Rust operating system whose user identity is the post-quantum chain identity, and whose packages are reproducibly built and signed | `crates/build-guard`, ADR-080 provenance | research, ADR |
| attested-hardware | Boot and device attestation that a node or wallet can prove | `hal/`, `crates/tiers` | TEE invariant first (CLAUDE.md) |
| compute-fabric | One market for CPU, GPU and later other compute, verified per job | ADR-079 | ADR-079 phases |
| long-archive | Records that outlive their media (DNA and optical archival research) | dna-quaternary-archival-engine (planned) | research |
| interplanetary | Delay-tolerant links and timing across light-minutes | ccsds-delay-tolerant-networking-bpv7 (planned) | invariant 9 (heights, never timestamps) reconciliation |
| self-governing-evolution | Pre-shipped code turned on by vote, never code loaded from state | ADR-078 (Omega), ADR-082 | ADR-078 acceptance; invariant 13 (no governance value is a program) |

## I. Horizon ideas from science fiction (Eric 2026-11-10)

Films have shown things that later became real. Each idea below starts from
a need such a film points at, not from the gadget. **Rung** says how far it
is today: R0 = buildable with what exists now, R1 = needs engineering we do
not have yet, R2 = needs a research result. **Every row is unverified for
novelty**: before any "others don't have this" wording it needs a dated
`docs/prior-art/` search. Some already have known prior art, noted where we
know it.

| id | Need (inspired by) | Rung | Builds on | Known prior art / gate |
|---|---|---|---|---|
| time-capsule | Messages and payments that open at a future height, sealed until then (time-travel films) | R0 | timelock encryption, block heights (invariant 9) | [prior art](../prior-art/time-capsule.md): drand tlock; differentiator = chain-committed release height, PQ sealing is research |
| digital-will | Assets and messages that pass to heirs if the owner stops signing (many dramas) | R0 | inheritance (B), smart account | [prior art](../prior-art/digital-will.md): Webacy, Safe/Argent recovery; legal wording open |
| babel-chat | Real-time translation inside end-to-end encrypted chat, on the device (the universal translator of space operas) | R1 | chat, on-device-assistant | [prior art](../prior-art/babel-chat.md): on-device models; data-leaves-device statement |
| personal-agent | An AI aide that runs errands with hard spending limits and asks before large acts (the household AI of superhero films) | R0 | agent-wallets, mcp-server | [prior art](../prior-art/personal-agent.md): Coinbase Agentic Wallets; ADR (limits, kill switch) still to write |
| human-or-machine | Proving a person is a person without biometrics on chain (the replicant test) | R1 | personhood | [prior art](../prior-art/human-or-machine.md): privacy red team |
| avatar-passport | One identity and owned items across virtual worlds and games (VR films) | R1 | identity-app, rwa | [prior art](../prior-art/avatar-passport.md): interop standards survey |
| fabrication-rights | Licensed, traceable files for 3D printers and future fabricators (the replicator) | R0 | notary, rwa, render-market | [prior art](../prior-art/fabrication-rights.md): two US patents; provenance only, never lock-in |
| health-vault | Medical records the patient owns and grants per visit (the medical bays of space films) | R1 | genetics-vault, identity-app | [prior art](../prior-art/health-vault.md): health-data law per country |
| foundation-archive | Civilisation's knowledge kept verifiable for centuries (the galactic encyclopedia) | R1 | long-archive, research-records, `crates/da` | [prior art](../prior-art/foundation-archive.md): archival formats; long-term hash migration via the suite registry |
| light-lag-ledger | Payments between planets minutes apart, settled when links return (space operas) | R2 | interplanetary, `crates/l2-flash` | [prior art](../prior-art/light-lag-ledger.md): invariant 9; DTN research |
| mind-link-safety | Consent and logging for brain-computer interfaces (neural-link films) | R2 | `hal/bci-sim` (SIM) | [prior art](../prior-art/mind-link-safety.md): SIM only; never on a production path (Standing Order 3) |
| swarm-coordination | Many robots or drones agreeing on tasks without a central controller (swarm films) | R1 | `hal/swarm`, dag-bft research | [prior art](../prior-art/swarm-coordination.md): civil uses only |
| city-mesh | Phones keep messaging and paying when the network goes down (disaster films) | R1 | mesh-network, light client | [prior art](../prior-art/city-mesh.md): spectrum rules |
| energy-credits | Measured, verifiable energy sharing between neighbours (solar-punk futures) | R1 | `hal/energy`, iot-anchor | [prior art](../prior-art/energy-credits.md): metering law |
| quantum-ready-keys | Keys that rotate to new algorithms with no user action (the quantum threat in thrillers) | R0 | suite registry, smart account | [prior art](../prior-art/quantum-ready-keys.md): Algorand Falcon, XRPL RegularKey; silent-rotation UX unbuilt |
| guardian-ai | A defence AI that watches the network and proposes fixes but can never act alone (the rogue-AI warnings) | R0 | sentinel-ai, ADR-077 | [prior art](../prior-art/guardian-ai.md): Forta, Hypernative; ADR-077 consensus boundary |
| truth-stamps | Proving a photo or video is original and unedited since capture (deepfake thrillers) | R1 | notary, attested-hardware | [prior art](../prior-art/truth-stamps.md): C2PA exists; differentiator would be PQ signing at capture |
| emergency-identity | A portable, recoverable identity for people who lost their papers (refugee films) | R1 | `crates/identity`, `crates/smart-account/src/recovery.rs` | [prior art](../prior-art/emergency-identity.md): ID2020, Sovrin/Indy, UNHCR/WFP Building Blocks; no biometric on chain; gate: GDPR and UNHCR data-protection review |
| autonomous-vehicle-payments | Vehicles pay tolls and charging inside limits they cannot exceed (self-driving films) | R1 | `crates/smart-account/src/policy.rs`, `hal/swarm` | [prior art](../prior-art/autonomous-vehicle-payments.md): IOTA/Bosch/MOBI and JLR concepts, no production found; gate: safety certification |
| space-debris-registry | One public registry of objects in orbit with owners and manoeuvre history (orbital-clutter films) | R2 | `crates/rwa`, `crates/da` | [prior art](../prior-art/space-debris-registry.md): UN register, LeoLabs, academic proposal; a mirror of state records only; gate: Outer Space Treaty and orbital data sources |
| seed-vault-attestation | Proof that a crop seed sample is held, viable and genetically distinct (seed vaults in survival films) | R1 | `crates/da`, `crates/archive` | [prior art](../prior-art/seed-vault-attestation.md): Svalbard (no chain), arXiv 2501.15962 PoC; attests lab records, not viability; gate: lab certification and Nagoya/ITPGRFA DSI |
| aid-tracing | Disaster-relief funds traced to recipients, with allow-lists and limits (relief-convoy films) | R1 | `crates/smart-account/src/policy.rs`, `crates/rwa` | [prior art](../prior-art/aid-tracing.md): WFP/UNHCR Building Blocks (permissioned Ethereum); gate: NGO, legal and recipient-privacy review |

The list is open: new ideas are added as rows here, each with its need, its
rung and a prior-art check, and they wait their wave like everything else.

## J. Trust you can verify (research 2026-11-10)

Each row turns a statement the foundation makes today into a check anyone can
run. *Builds on* names paths verified in the tree on 2026-11-10. *Differs*
lists only differences backed by repo facts; prior art is in the research notes linked
after the table. Rungs and waves as in sections I and Waves. **Independence is not
claimed by any row**: until a party other than the foundation runs the
rebuilder, replayer, witness or monitor, these are tools that let others
check, not checks others have made.

| id | Need | How it works | Builds on | Differs from prior art | Rung | Wave | Size | Consensus |
|---|---|---|---|---|---|---|---|---|
| rebuilder-attestations | A release is trusted because outsiders rebuilt it, not because CI says so | Anyone runs `scripts/repro_build.sh` in a digest-pinned container and signs `(tag, file, sha256)` with an ML-DSA key; signatures land in a public `release-attestations/` repo by PR; `cargo xtask verify-release --rebuilders k` counts matching signers | ADR-054, `scripts/repro_build.sh`, `.github/workflows/reproducible-build.yml`, `xtask/src/verify_release.rs`, the `fips204` dependency | Builder signatures are post-quantum (ML-DSA) rather than PGP; one verifier command already checks cosign + SLSA per file (ADR-080) and gains a k-of-n rebuilder count; closes ADR-054's "different runner images" gap by pinning the C toolchain. Includes N's rebuilder-quorum: validators may sign with their ML-DSA-87 keys | R0 | W0 | M | No |
| release-log-anchor | A swapped or back-dated release is caught even if GitHub or Rekor is compromised | Each release's `SHA256SUMS` hash is anchored on chain via the notary mechanism; the update checker (ADR-075) compares a fetched manifest with the anchored hash and warns, never installs. No node refuses to run (self-checking binaries are circular) | ADR-080, `bins/maya2c-node/src/update_notice.rs`, notary, `crates/light-client` | Log consistency from the chain's own headers instead of a separate log operator; complements Rekor | R0 | W1 | S | No (existing tx types; confirm notary design) |
| claims-ledger | Every number on the site and in announcements traces to the run that produced it | Each figure gets an evidence id: commit, command, hardware, OS, CI run URL, raw output path under `reports/`; the site renders numbers only from those records; `claims-check` fails on a bare number on a public surface | `xtask/src/claims_check.rs`, `xtask/src/evidence.rs`, `xtask/src/go_no_go.rs`, `reports/bench/` | Extends an existing CI gate (superlatives) to numbers; prior art found is benchmark dashboards, not marketing-copy gates (gap in search, not a novelty claim) | R0 | W0 | M | No |
| public-invariant-monitor | Anyone can watch for a conservation break or halt without trusting the foundation's watcher | Package `apps/sentinel` detectors as a standalone monitor polling several RPC endpoints, cross-checking tip, state root and supply, alerting (ntfy or stdout) on disagreement or a guard trip | `apps/sentinel/src/detect.rs`, `incident.rs`, `crates/node/src/state/invariant_guard/mod.rs`, `docs/runbooks/invariant-guard-tripped.md` | Conservation already refused in-node (invariant 28); the monitor re-derives it from outside and compares sources, as CT monitors do | R0 | W1 | M | No |
| supply-recompute | Total supply is a figure anyone recomputes, not one the RPC asserts | `maya2c supply-audit --height H`: fetch snapshot manifest/chunks, import and check root vs `header(H).state_root`, sum balances + pool + staking with checked U256 adds, compare to genesis allocation + `epoch_reward` schedule − burns; non-zero exit on any difference; asserts shielded pool 0 while its activation is `u64::MAX` | `crates/node/src/state_pruner/snapshot.rs`, `crates/node/src/rpc/market.rs:85-116`, `crates/node/src/state/mining.rs:208-268`, `crates/node/tests/supply_property_tests.rs` | Compares to the emission schedule, not only a total; checked adds where `get_supply` uses `saturating_add` (`market.rs:95,97,108`). A per-block header supply commitment is optional later consensus work via ADR-082. This is the one supply tool: M `supply-audit` (b) and N's supply-audit point here | R0 | W0 | S | No (header commitment would be, ADR-082) |
| replay-anyone | Any block's effects reproducible by anyone with one command | Register the existing `maya2c replay` in `features.toml` with a test; add `--range a..b`; replayers may sign `(height, state_root, binary sha256)` and publish, tied to rebuilder-attestations | `bins/maya2c-cli/src/fork.rs`, `crates/node/src/state/db.rs:1475` (`apply_block_checked`), invariant 24 | Replay already refuses a non-reproducing root (invariant 24); prior art replays test fixtures, this replays live blocks from a verified snapshot | R0 | W0 | S | No |
| ceremony-transcript | Genesis can be re-derived and checked by anyone, step by step | Publish the full transcript: binary hash, every input (validator pubkeys, payout addresses, council keys, timestamp, difficulty), commitment sheet, genesis id; operators sign the sheet with ML-DSA-87; `cargo xtask verify-ceremony <dir>` recomputes the genesis id and runs `genesis-check` | `bins/genesis-ceremony/src/main.rs`, `xtask/src/ceremony_rehearsal.rs`, `xtask/src/genesis_check.rs`, `deploy/genesis/mainnet/README.md` | No toxic waste (transparent proofs, ADR-008), so verification is full recomputation rather than a 1-of-N honesty assumption as in KZG ceremonies | R0 | W0 (before genesis) | S | No (genesis-permanent output) |
| signed-incidents | Outages and attacks recorded so the foundation cannot quietly edit them later | Hash-chained signed incident records (heights, block ids, RPC captures, timeline, fix commit); record hash anchored via notary; includes a replay command line | `apps/sentinel/src/incident.rs`, `reports/incidents/`, `docs/runbooks/`, ADR-077, status-v2 incident store | Records point at heights `replay-anyone` can re-execute | R0 | W1 | S | No |
| validator-ledger | Validator behaviour (signing, missed rounds, equivocation, stake concentration) public and derived only from signed consensus data | Page + CLI built only from BFT certificates, `get_validators`, equivocation evidence — never self-reports (invariant 14); needs one RPC: epoch-frozen weights (`decentral.rs` notes the gap) | `apps/status/src/decentral.rs`, `crates/node/src/consensus/bft/evidence.rs`, `get_bft_status`, invariant 14 | Excludes unauthenticated telemetry by rule | R0 page / R1 RPC | W1 | M | No (RPC is a node change: after the freeze) |
| foundation-canary | Users learn if the foundation is compelled or compromised | Signed statement every N blocks including a recent block hash (cannot be pre-signed), ML-DSA, anchored via notary; `public-invariant-monitor` alerts on lapse | notary, the `fips204` dependency, `apps/sentinel/src/notify.rs` | Freshness proven by a block hash, lapse detection automatic | R0 | W1 | S | No (legal wording review) |
| witnessed-checkpoints | A light client can tell if it is shown a split view | Serve `AccountProof` and `up:` schedule proofs over RPC (closes ADR-082 M2); independent witnesses cosign `get_checkpoint` outputs after consistency checks; light clients require k cosignatures | `crates/light-client`, `get_checkpoint`, ADR-082 M2, BACKLOG entry | Addresses the eclipse case `state_pruner/snapshot.rs` names with signed witnesses instead of peer sampling | R1 | W2 | M | No for witnesses; RPC is a node change |

Rejected: "nodes refuse unknown binaries" — a binary that checks its own hash proves
nothing when tampered with; needs `attested-hardware` (H) first. Deferred to its ADR:
k-of-n threshold release signing (ADR-080 follow-up), meaningful only once the k key
holders are different people.

Prior-art notes for this section: [research/trust-j-trust-you-can-verify-2026-11-10.md](../research/trust-j-trust-you-can-verify-2026-11-10.md).

## K. Protection for people (research 2026-11-10)

Repo facts (checked 2026-11-10): vault-accounts REAL/verified (invariant 32, features.toml ~3432) but no
wallet or CLI builds Configure/Request/Cancel — only tests (`crates/node/tests/vault_tests.rs`,
`bins/maya2c-cli/tests/vault_devnet_tests.rs`); `bins/maya2c-cli/src/vault.rs:16` reads status via
`vault_get` only. clear-signing-review RESEARCH ("no wallet simulates contract calls yet");
`apps/wallet-gui/core` does not depend on `clear-sign` or `smart-account`. smart-account RESEARCH, not a
node tx type. No simulate/dry-run RPC (`crates/node/src/rpc/mod.rs:9-24`).
`THREAT_INTEL_ACTIVATION_HEIGHT = u64::MAX`. **Finding:** `crates/clear-sign/src/lib.rs:195-201`
compares raw bytes `[..2]` and `[30..]` — that matches hex display, not Bech32m, whose last 6
characters are the checksum; a poisoner grinding the visible Bech32m head and tail gets no
`LookAlikeRecipient` warning. Rows already in the catalog are not repeated (wallet-v2, inheritance,
digital-will, agent-wallets, personal-agent, names, recurring-payments).

| id | What, for whom | Builds on | Gate | Size | Wave | Rung |
|---|---|---|---|---|---|---|
| undo-send | Opt-in "undo send": a payment waits N blocks in escrow, the recipient sees it pending, the sender cancels until the stated height | ADR-030 Request/Cancel/Execute (vault limit = 0), `vault_get`, wallet core `compose.rs` | wallet ADR for UX; no consensus change; UI stops offering undo one block before `ready_height` (ADR-030:91 race) | M | W1 | R0 |
| payee-check | Payee confirmation like UK CoP / EU VoP without a central name database: the payee signs a label in the payment request; wallet shows Match / Close match / No match against the label and contacts | `payment.rs` PaymentRequest, `crates/address`, wallet-v2 contacts | ADR (label format), privacy statement; UI must say it defeats substitution, not impersonation | S–M | W1 | R0 |
| sim-firewall | Simulate before signing; compare with what the app claimed; block a mismatch until read | `crates/clear-sign` `review()`, `describe.rs`, new read-only `simulate_transaction` RPC | node release (RPC, not consensus); trust model for the simulating node (two nodes or light-client-verified state) | M | W1 | R0 |
| poison-shield | Fix the Bech32m look-alike check; no copy-from-history; contacts-only autocomplete; look-alike incoming transfers collapsed and labelled | `crates/clear-sign/src/lib.rs:195`, `crates/address`, wallet history | none | S | W0 | R0 |
| second-screen-check | A second device reads open vault requests directly from the chain and shows them with Cancel, so a compromised desktop cannot change what it shows | ADR-030 guardian Cancel, `vault_get`, `describe_vault` (`describe.rs:540`), wallet-mobile | wallet-mobile | M | W1 desktop / W2 phone | R0 |
| panic-sweep | Panic button: phase 1 cancel every open request and alert guardians; phase 2 send escrow and balance to a recovery address committed under the reconfiguration delay | ADR-030, `MAX_OPEN_REQUESTS` (`vault.rs:33`), notifications | phase 1 none; phase 2 ADR, spec, vectors, invariant 32 review, `RECORD_VERSION` bump, ADR-082 entry | S / M | W1 / W2 | R0 / R1 |
| permission-center | One screen of every live allowance, session key and limit with one-tap revoke; expiry by default; no "unlimited" without typed confirmation | `SessionScope`, `Policy` (smart-account), clear-sign `UnlimitedApproval`, `PermitSignature` | smart-account as a node tx type | M | W2 | R0 |
| cooling-off | Large first payment to a new payee waits a short timer, then goes through undo-send; prompt asks bank-style push-fraud questions | undo-send, contacts, later `Policy.delay_above` | wording review; measure effect before claiming any | S | W1 | R0 |
| scam-registry | Signed scam reports kept off chain; wallets subscribe to curators they choose; accused notified and can appeal; reports expire; warn only, never block | threat-intel signed-attestation format (not the `t:` state) | abuse policy, appeal process, defamation review | M | W2 | R0 |
| intent-descriptors | Plain-language contracts: each VM contract ships a descriptor wallets render (ERC-7730 analogue), attested by deployer and chosen curators | clear-sign `Effect::Call { method }`, `crates/contract-safety`, verified-source set | ADR (format, trust model), off chain first | M | W2 | R1 |
| mutual-cover | Member-owned cover pool for theft losses with public claims; vault custody reports as evidence | `bins/maya2c-cli/src/custody_report.rs`, `crates/governance`, VM contract | insurance law per country (Eric), actuarial review | L | W4 | R1 |

Prior-art notes for this section: [research/trust-k-protection-for-people-2026-11-10.md](../research/trust-k-protection-for-people-2026-11-10.md).

## L. Crypto that outlives us (research 2026-11-10)

Every row is unverified for novelty; each needs a dated `docs/prior-art/` file before
any "others don't" wording. Effort S = days, M = weeks, L = a month or more.

Repo facts relied on (checked 2026-11-10): six suites in `crates/crypto-pq/src/suite/mod.rs:47-60`
(0x01 Ed25519, 0x10/0x11 ML-DSA-65/87, 0x20 SLH-DSA-SHA2-128s, 0x21 SLH-DSA-SHAKE-256f,
0x30 hybrid); `agility/migration.rs:1-21` (rotate, commit_recovery, sweep_step, vault) is not
called outside crypto-pq, and ADR-082:195 says account migration for a sunset suite is not
built; `agility/audit.rs:51` flags suites only by static `pq_security_bits`;
`crates/node/src/network/pq/mod.rs:38-48` "Authentication stays classical";
`MlKem1024Policy::Required` not wired to config (features.toml ~1804); dual-KEM ML-KEM+HQC off
by default, HQC decapsulation not constant-time (dudect |t| = 47.5, features.toml ~2960);
`crates/vrf` is ECVRF-EDWARDS25519 (classical), consumer the oracle registry; every chain hash
is BLAKE3 (`core/block.rs:301,319,401`, `core/transaction.rs:564`, `crates/archive/src/car.rs:33`)
with no hash suite; no Kani harness in `crates/crypto-pq` or `crates/node/src`.

Doc drift found and fixed in the same change: `docs/invariants.md:259` said
`MIN_COUNCIL_SUNSET_LEAD_BLOCKS` is "7 days"; code `crates/node/src/state/upgrades.rs:39` has `COUNCIL_SUNSET_LEAD_BLOCKS = MAX_DELAY_BLOCKS +
MIN_MIGRATION_WINDOW` = 1,172,800 blocks (~13.6 days), matching ADR-082:86. `MIN_MIGRATION_WINDOW`
is 172,800 blocks = 2 days (`agility/mod.rs:29`); ADR-082:94-96 leaves it open before mainnet
(BIP-361 gives years).

| id | Need | Mechanism (repo paths) | Prior art | Maya2C difference (repo fact) | Rung | Wave | Effort | Consensus | Risks |
|---|---|---|---|---|---|---|---|---|---|
| quantum-canary | Public, costly signal that a hardness family is breakable, tied to a pre-agreed response | Ladder of deliberately weakened puzzles per family (planted-secret LWE, small-curve ECDLP, truncated-hash preimage) with integer-only dependency-free checkers (invariants 1, 6 rule); commit-reveal claim (H(solution ‖ claimer), reveal N blocks later) writing a `cn:` record under a new committed layer (invariant 25); top rung auto-schedules a sunset of that family's single-family suites at claim height + `COUNCIL_SUNSET_LEAD_BLOCKS`; lower rungs only alarm. Includes N's off-chain variant: a watcher on `crates/btc-spv` + `crates/deposit-watcher` following a Bitcoin canary and triggering `docs/security/Q_DAY_PLAYBOOK.md` (funding a BTC canary needs APPROVED) | Todd 2013 SHA1/SHA256/RIPEMD bounties (SHA1 claimed 2017 after SHAttered was public); BitMEX canary fund 2026-04 (secondary); Project Eleven Q-Day Prize 2026-04 (contested) | 0x30 is an AND-hybrid, so a lattice-only claim retires 0x10/0x11 while 0x30 stays valid on its SLH-DSA half — no chain-wide emergency; sunset machinery exists (ADR-082 `up:s`) | R0 checker / R1 params | W2 | L | Yes (tx kind, prefix, ADR-082 writer) | Checker bug = open money tap (Kani first); griefing bounded to single-family suites; silent state actors; front-running (commit-reveal); bounty may be too small; invariant 34 ordering |
| sunset-migration | When a suite sunsets, funds move to a surviving key without a hard fork or trusting the broken key | Wire `agility::migration` into apply behind an activation height; then the PLANNED STARK proof of seed knowledge (`migration.rs:18-21`) on transparent-stark-plonky3 for accounts that registered nothing | Buterin 2024 ethresear.ch quantum-emergency hard fork (STARK over BIP-32 seed); BIP-361 (draft) | Engine and tests exist; keys derive from `MasterSeed` by hash `expand` (`suite/mod.rs:422`), so a seed-preimage STARK is hash-only | R0 wiring / R1 STARK | W2 | L | Yes | Sweeping dormant funds is policy; vault limits (invariant 32); 2-day window too short |
| backup-key-commit | Survive a family break even if offline | Expose `commit_recovery` as a cheap tx registering H(pk′) of a backup key in a different family (e.g. 0x21 for a lattice-primary account); wallet "add a quantum backup" | as above | `commit_recovery` and vault "opens only to the committed key" exist (`migration.rs:13-16`) | R0 | W2 | S-M | Yes (small) | Lost backup = no backup; commitment hash must not be BLAKE3-only |
| level5-hybrid | Long-held value wants margin beyond category 1–3 | New SuiteId 0x31 = ML-DSA-87 + SLH-DSA-SHAKE-256 as an AND-combiner, ACVP vectors (invariant 29), opt-in; triple hybrid waits for final standards | FIPS 204/205 final; FIPS 206 not final; onramp round 3 (FAEST, HAWK, MAYO, MQOM, QR-UOV, SDitH, SNOVA, SQIsign, UOV) | Both components already compiled (0x11, 0x21); adds hash diversity (SHA2 in 0x30 vs SHAKE) | R0 / R2 third family | W2 | M | Yes (new suite) | Signature size vs 5 MiB block cap (invariant 33) — measure; availability AND across more implementations |
| hash-agility | Every commitment rests on one hash family with no scheduled retirement | ADR first. (a) dual-hash BLAKE3 ‖ SHA3-256 in archive manifests and checkpoints; (b) later a hash-suite byte in tx/state-root contexts — a flag-day per ADR-082:176-179 | RFC 4998 hash-tree renewal; draft EP-evidence-record | Suite registry covers signatures/KEMs only; no hash suite exists | R1 | W1 (a) / W4 (b) | M / L | (a) none / (b) flag-day | Insurance only (no evidence against BLAKE3); verifiers must check both |
| evidence-records | Archived history stays verifiable after its signing suite is sunset | Periodic re-attestation of archive roots with the committee ML-DSA-87 key under a new FIPS 204 context plus a hash-based signature, chained per RFC 4998; mandatory renewal on sunset (`crates/archive`, `crates/light-client`) | RFC 4998; draft-alhemeiri-wathiqa-pqc-ers | Archive content-addressed with self-checking CIDs; validator keys ML-DSA-87 with contexts (`suite/validator.rs`) | R0 | W1 | M | None | New signing context needed; committee key history |
| pq-peer-auth | Close the documented active-impersonation gap | Wire `MlKem1024Policy::Required` to operator config; sign the handshake transcript with the validator ML-DSA-87 key for committee peers; keep dual-KEM off until FIPS 207 final and HQC timing fixed | X-Wing draft -11 (Informational); CiC 2024 proof | HNDL confidentiality covered (`pq/mod.rs:30-36`); this is the authentication remainder | R0 | W1 | M | None (version-negotiated p2p) | Compatibility with old peers; context reuse must not let a handshake signature verify as a block signature |
| pq-randomness | Randomness unpredictable after curve25519 falls | Phase 1 mix ECVRF output with committee commit-reveal over BLAKE3; phase 2 replace with a mature PQ VRF (`crates/vrf`, oracle registry) | LaV (Crypto'23), X-VRF, DeuringVRF (Eurocrypt 2025), ePrint 2025/766 | `vrf/src/lib.rs:10-25` states it is classical; consumer is the optional oracle (invariant 11) | R1 / R2 | W2 / W4 | M / L | Yes (oracle selection) | ML-DSA/SLH-DSA signatures not unique → not VRFs; withholding bias |
| algo-health | Holders see, dated and sourced, how each suite stands | Signed dated `suite-health.toml`: NIST status, ePrint cryptanalysis ids, best quantum estimate, constant-time measurements, library versions, RUSTSEC hits; shown in status/explorer/wallet; never read by consensus (invariants 11, 13, 14) | Project Eleven PQ registry (vendor); GRI Quantum Threat Timeline | `audit.rs:51` hook exists; ledger already records side-channel results | R0 | W0 | S | None | Hype surface — every figure dated and sourced, marked unverified |
| key-age-warnings | Wallets nudge rotation before a sunset | Wallet/RPC read `up:s` sunset records and the account suite; warn on scheduled sunset, missing backup commit, or key older than an advisory cryptoperiod | NIST SP 800-57; BIP-361 phase A | Sunset records are committed state (`state/upgrades.rs:42-47`) | R0 | W1 | S | None | Light clients need the next row |
| light-sunset-witness | Light clients and wallets learn upgrades and sunsets without trusting the caller (includes M's schedule-witness) | State proof of `up:` records against the trusted root in `LightClient::verify_block` (ADR-082 M2) | sync-committee light clients | Gap documented (ADR-082:160-169, BACKLOG) | R0 | W1 | M | None | Wrong proof path accepts a stale schedule |
| hybrid-proofs | "Two signatures" is only as strong as combiner and parser | Kani harnesses on 0x30 split/parse (no panic, injective concatenation, both verifiers always run) and the canary checker (Kani in WSL); written reduction that the AND-combiner is EUF-CMA if either is, plus stripping: a 0x30 half presented as 0x10/0x20 maps to a different `suite_address` (`suite/mod.rs:313`); EasyCrypt later | Bindel et al. 2017; draft-ietf-pquip-hybrid-signature-spectrums; X-Wing CiC proof | Today 4-angle tests (`crates/node/tests/hybrid_tests.rs`), zero Kani harnesses in crypto paths | R0 Kani / R1 EasyCrypt | W0 | M | None | A parser proof does not prove ML-DSA/SLH-DSA; never call it "formally verified" beyond what was proved |
| lms-release-signing | Signed updates stay trustworthy past a curve break | LMS signature (SP 800-208 / RFC 8554) alongside cosign on releases and the in-app update channel; state on one offline machine; never for transactions | CNSA 2.0 LMS/XMSS for software signing (vendor-sourced dates); leanSig | Releases already through cosign; owner directive asks for a signed update channel in every app | R0 | W1 | M | None | State reuse = forgery; LMS crate needs cargo vet; single offline signer |

Prior-art notes for this section: [research/trust-l-crypto-outlives-us-2026-11-10.md](../research/trust-l-crypto-outlives-us-2026-11-10.md).

## M. Rules nobody can bend (research 2026-11-10)

Problems found (Standing Order 9), verified against master 2026-11-10. F1 and
F2 are in the frozen node binary, so they are recorded in `BACKLOG.md` (P1,
decision for Eric: re-freeze or fix outside the node), not fixed. F3 and F4
were documentation only and are corrected in the same change as this section.

| # | Where | What fails |
|---|---|---|
| F1 | `crates/node/src/rpc/market.rs:48` | `NON_CIRCULATING` lists only `FEE_SINK`. The keyless mining reserve (`MINING_RESERVE`, `crates/node/src/state/mining.rs:41`) holds ~1.23×10^17 coins at genesis (`deploy/genesis/mainnet/README.md`, `--mining-reserve 122753829695359067419575633036805965021648` base units). `get_supply`, `/api/v1/supply` and aggregator endpoints would report it as circulating on day one, while first-era issuance is 19.69M. Not consensus, but in the frozen node binary. |
| F2 | `crates/node/src/core/describe.rs:336-343` | `TxKind::Propose` is described by parameter count only and ignores `ProposalSubmission.upgrade` (`core/governance_payload.rs:156`): an upgrade-only proposal shows "Propose changing 0 parameters…" — blind signing on the most powerful tx kind. The no-wildcard match does not catch a field inside an existing variant. |
| F3 | `docs/invariants.md:254-262` | Invariant 34 says the council lead is 7 days, names `MIN_COUNCIL_SUNSET_LEAD_BLOCKS` and places it in `crates/governance/src/limits.rs`; code is `COUNCIL_SUNSET_LEAD_BLOCKS` in `crates/node/src/state/upgrades.rs:39` = 1,172,800 blocks (~13.6 days, DECISIONS 2026-11-07, ADR-082). Corrected in `docs/invariants.md`. |
| F4 | `docs/ecosystem/CATALOG.md` §G | "A council that sunsets by itself" — `CouncilGenesis` (`crates/node/src/genesis.rs:154-161`) has members, threshold, `max_pause_blocks`; pauses expire, the council has no term. The "sunset" under ADR-082 is the council's power to schedule *suite* sunsets (`CouncilKind::SunsetSuites`), not a sunset of the council. Row reworded in §G. |

Smaller: supply sum uses `saturating_add` (`market.rs:95,97,108`) — overflow would understate silently.
Nothing pins `MIN_UPGRADE_LEAD_BLOCKS` (2,592,000) ≥ `vault::MAX_DELAY_BLOCKS` (1,000,000) + other exit paths.

Consensus column: none = app/RPC only; AH = needs an activation height.

| id | Need | Mechanism (builds on) | Maya2C difference (repo facts) | Rung | Wave | Size | Consensus | Main risk |
|---|---|---|---|---|---|---|---|---|
| exit-window | Locked funds can exit before a rejected rule takes effect | `const` check in `governance/limits.rs`: `MIN_UPGRADE_LEAD_BLOCKS >= vault::MAX_DELAY_BLOCKS + governance unlock + unbonding window`, derived like the council lead; RPC `get_schedule` shows "last safe exit height" per pending entry | Lead already 30 days from execution height (invariant 34), like L2BEAT Stage 2; entries cannot be cancelled or moved; missing the derivation and its pin | R0 | W0 check / W1 RPC | S | none | On an L1 a holder's real exit is selling or not upgrading — say so |
| upgrade-explainer | Plain-words view of what an upgrade switches on before voting or signing | Fix F2 (version, activation height, each FeatureSet bit by name, each suite sunset); proposal page in explorer-v2; AI summary only after a named human signs it, marked advisory (ADR-077) | `describe.rs` is the single source for wallet, explorer and CLI, with a no-wildcard match | R0 | W0 (F2) / W2 (AI) | S + M | none | AI summary trusted over the diff — show the diff first |
| supply-audit | Anyone checks that no coins were printed | (a) Fix F1 (reserve non-circulating; checked_add); (b) the standalone recompute is J `supply-recompute`, extended to assert total = genesis reserve + bonds and that the reserve fell by exactly the scheduled `epoch_reward` payouts; (c) public curve page | Rewards paid from a reserve fixed at genesis, never minted; conservation checked in consensus (`invariant_guard/conservation.rs`); none of the 10 `ParameterKey`s (`governance/src/params.rs:61`) touches emission | R0 | W0 (F1) / W1 | S + M | none | Tool must not share node code — depend only on `ledger-math` and the spec |
| founder-ledger | See what the founder holds and how fast it can be spent | Publish founder addresses in the genesis README; hold coins in a vault account (ADR-030) with public limit per delay and guardian-cancellable large withdrawals; dashboard; a hard "lock until height" is a new tx kind (AH) | Founder mining disclosed (`DECISIONS.md`, ADR-044); vault rate limit pinned by `vault_tests.rs` | R0 / R1 lock | W1 / W2 | S / M | none / AH | Covers declared addresses only — say so |
| reserves-proof | Exchanges and bridges listing MAYA prove they hold what they owe | Rust SDK/CLI building a Merkle-sum tree of liabilities, root signed with ML-DSA, assets checked against state proofs of custody addresses, root anchored via notary | PQ signature on the root (needs prior-art search); state proofs from the committed root (invariant 25) | R0 / R1 ZK | W2 | M | none | Plain sum trees leak neighbouring balances |
| equivocation-watch | Anyone proves a validator cheated, publicly | Today a node writes evidence to disk and an operator files it (`consensus/bft/evidence.rs:1-11`); add submission by any watcher, explorer page of slashings with raw frames, double votes/attestations as evidence; reporter reward is Eric's economics call | `ReportEquivocation` fileable by any account, calls `slash_double_sign` (`state/staking_exec.rs:208-211`); no reporter reward found | R0 / R1 | W1 / W2 | M | AH for new evidence kinds or reward | False-evidence flood — red-team |
| constitution | Short list of rules no vote or council can change, each checkable | `docs/constitution.md` lists only compiled, pinned rules (invariants 12, 13, 32, 34; no emission ParameterKey; council limited to pause and sunset, `state/council.rs:1-14`), each line pointing to its test; CI fails on a line without a pinning test | Compiled bounds under invariant 12, not promises | R0 | W0 | S | none | A hard fork can always change any of it — the text must say so |
| holder-veto | Stakers and holders can delay or block an upgrade they reject | Dual governance: lock coins in escrow during the timelock; above threshold 1 the timelock grows, above threshold 2 the proposal expires (ADR-077 D3 "holder veto as a second house", approved) | Voting weight = locked stake + work credit (`governance/src/tally.rs:15`); executed entries never cancel (invariant 34), so the veto acts before execution only | R1 | W3 | L | AH, new prefix, ADR | Cheap-veto deadlock; Lido escrow bug report (Certora 2025-02) |
| treasury-ledger | Any shared fund is spent only on chain within public limits | Mainnet has no treasury (`deploy/genesis/mainnet/README.md`) — explorer says so; if added later wire `crates/treasury` behind governance | `crates/treasury` tested, unwired | R0 / R1 | W1 / W4 | S / L | AH for wiring | Adding a treasury is an economics change (Eric + ADR) |
| mempool-stats | See that block order is not exploited | Per-block stats: share sealed vs clear, decryption-share timing, any sealed tx left out after reveal | Threshold-encrypted mempool shipped (`node/src/sealed/`, `state/sealed_exec.rs`); optional at genesis, mainnet committee not verified; its KEM is Ristretto ElGamal, not post-quantum (architecture-vision) — the page must say so | R0 | W1 | M | none | Show "off" when off |
| upgrade-readiness | Everyone sees before height H whether enough validators run the new binary | Validators advertise `SUPPORTED_PROTOCOL_VERSION` in the p2p handshake; page shows ready share per pending H; advisory, never a trigger | Above f non-upgraded validators the chain stalls, not forks (pinned in ADR-082 tests); rejoin after halt NOT VERIFIED (ADR-082:139) | R0 | W1 | S | none | Self-reported — label "claimed" |

Merged elsewhere: schedule-witness (a state proof of `up:` records in
`LightClient::verify_block`, ADR-082 M2) is the same item as L
`light-sunset-witness`.

Prior-art notes for this section: [research/trust-m-rules-nobody-can-bend-2026-11-10.md](../research/trust-m-rules-nobody-can-bend-2026-11-10.md).

## N. Out of the box (research 2026-11-10)

12 kept (nine below, three merged into J, L and M), 13 rejected. Repo findings: the VM import set is nine functions, none moves value
(`crates/node/tests/exploit_replays.rs` header) — no automatic on-chain bounty payout without a consensus
change; energy figures are modelled (`xtask/src/bench_report.rs:7-10`: joules = seconds × 45 W); the light
client does not check validity, so phone watchers depend on `crates/da` (RESEARCH). Rows overlapping
sections J and M (supply-audit, quantum-canary, rebuilder-quorum) are merged into those rows.

| id | Need | Mechanism (verified paths) | Rung | Wave | Size | Consensus | Prior art / difference / gate |
|---|---|---|---|---|---|---|---|
| flight-recorder | After an incident anyone replays the network's decisions and checks the same state roots | Incident bundle: CAR archive of the range (`crates/archive`, SLH-DSA seals `src/seal.rs`), BFT certificates (`consensus/bft/certsync.rs:292` `head_replay`, ADR-052), expected roots; an xtask replays through `apply_block_journaled` (invariant 24 refuses unreproduced roots); also makes DR drills replayable | R0 | W1 | M | None (offline) | Solana `ledger-tool verify` is a developer workflow; difference: sealed public bundle with pass/fail on the root. Gate: no peer IPs or secret-bearing logs |
| bench-rerun | Performance numbers as a spread of signed re-runs, not a best case | Operators run `cargo xtask bench-tps` at a named commit and sign the stamped report; publish median, worst and n | R0 | W2 | M | None | Chainspect, CertiK-refereed Sui claim, Venom standard proposal; no close match found for validator-signed re-runs (not a novelty claim). Early re-runs are not independent (single machine) — say so |
| explain-block | Anyone reads what a block or tx did, matching what the wallet showed | Run `crates/clear-sign` `render`/`review` over simulated effects for every tx in a block, served by `apps/explorer`; deterministic, no language model. **Built (W3-7):** `apps/explorer/src/explain.rs` renders each output through `render`; `review` is not run (the explorer has no claimed effects to compare against) | R0 | W1 | S | None | Loop Decoder, Etherscan decode, Solana IDL decoding; difference: one renderer for wallet, explorer and hardware wallet, from simulation not app claims |
| ai-receipts | Every AI answer says which model and input produced it | Receipt (weights hash, input commitment, output hash) signed with ML-DSA, anchored via notary, emitted by `bins/maya2c-mcp` and `crates/confidential-ai/src/attestation.rs` when a TEE exists | R0 commitment / R1 attested | W3 | M | None | IETF draft-tsyrulnikov-rats-attested-inference-receipt; the AFiR SPICE draft already uses ML-DSA-65, so PQ is **not** a difference; value is honesty that a commitment proves the claimed model, not that it ran. Gate: TEE invariant first |
| exploit-bounty-ledger | Researchers see a reproduced exploit gets paid without negotiation | Submission = failing test in the style of `exploit_replays.rs`, hash-committed first via notary; CI reproduces at a pinned commit; fixed test joins the proof-ratchet floor (`xtask/src/proof_ratchet.rs`); payout via `crates/treasury` staged approval — not automatic | R0 / R1 auto-pay | W2 | M | None (auto-pay needs VM value transfer) | Hydra framework (USENIX Security 2018, https://eprint.iacr.org/2017/1090); difference: exploit test becomes permanent memory in the ratchet. Gate: `APPROVED: bug-bounty` + funding |
| promise-ledger | Roadmap promises with a due height; misses reported automatically | Each promise a notarized hash with a due height linked to `docs/trajectory.md`; a go-no-go-style check (`xtask/src/go_no_go.rs`, `readiness.rs`) prints MET only with an evidence path, otherwise MISSED | R0 | W1 | S | None | Thin prior art; no close match for a chain's own roadmap (not a novelty claim). Risk: promising only easy things — show misses without spin |
| phone-watchers | Citizens run cheap watchers catching withheld data or bad encoding | `crates/light-client` + sampling and bad-encoding detection from `crates/da` (`src/lib.rs:22`, `Square::repair`); opt-in, bandwidth cap | R1 | W3 | L | Needs DA promoted from RESEARCH (activation height) | Celestia Lumina is a close match; difference: hash-only commitments, no KZG setup, PQ. Gate: platform compliance (no background drain) |
| censorship-monitor | Measured public evidence of exclusion per proposer | Per-hash arrival logs on watchers vs inclusion in DAG-BFT certificates; needs a hash-level feed (`get_mempool_info`, `rpc/server.rs:863`, is size/bytes only); alerts via `apps/sentinel` | R0 | W2 | M | None | Ethereum Censorship Monitor, Ultrasound dashboards, arXiv 2305.18545; must first check DAG-BFT attributes exclusion to one proposer; copy their honest-non-inclusion rules |
| trust-facts | Public sheet of the chain's trust computed only from evidence on disk, gaps scored as gaps | Combine `cargo xtask go-no-go`, `readiness`, `coverage`, `proof-ratchet`, latest sweep; each cell an evidence path or GAP; section G gaps first. **Built (W3-8):** `cargo xtask trust-facts` (`xtask/src/trust_facts.rs`) runs the four gates; the latest sweep is not yet folded in | R0 | W1 | S | None | L2BEAT Stages, DeFiScan; difference: generated by the repo's own gates. Self-assessment — label it; facts over one score |

Merged elsewhere: supply-audit → J `supply-recompute` (tool) and M `supply-audit` (F1 fix, curve page);
quantum-canary → L `quantum-canary` (its Bitcoin-canary watcher variant is noted in that row);
rebuilder-quorum → J `rebuilder-attestations`.

Rejected (13), each with its reason in the research notes linked below: among them a ZK proof that the
founder holds no hidden keys (non-possession cannot be proven), energy accounting (energy is modelled, not
measured), and eight ideas already covered by another row or tool.

Prior-art notes for this section: [research/trust-n-out-of-the-box-2026-11-10.md](../research/trust-n-out-of-the-box-2026-11-10.md).

## Waves

| Wave | When | Rule |
|---|---|---|
| W0 | Now, before genesis | Non-consensus only; the frozen node binary is not touched |
| W1 | Mainnet to +3 months | The stability window the investor condition requires: user apps and builder tools, no consensus changes |
| W2 | +3 to +6 months | Commerce, identity and operator tools; consensus items only at a written activation height |
| W3 | +6 to +12 months | The AI network, bridges and the fields |
| W4 | Later | Needs research, legal or hardware first |

## How to pick the next one

Highest wave first. Within a wave, in order: what unblocks the most other
entries (for example identity-app before reputation and genetics-vault),
then what users notice first, then what earns income (merchant-checkout,
sdks-1.0, PQ consulting). One entry at a time: build, debug, ship, then
the next.
