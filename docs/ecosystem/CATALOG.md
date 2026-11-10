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
| recurring-payments | Subscriptions and payroll with spending limits | `crates/smart-account` session keys | consensus review if a new state prefix is needed | M | W2 |
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
| names | Human-readable names for addresses | VM contract or a new state prefix | consensus review (prefix) or VM-only design ADR | M | W2 |
| notary | Timestamp and prove that a document existed (hash only, never the file) | transaction memo or VM contract | none if hash-only | S | W1 |
| reputation | Portable reputation for marketplaces and freelancers | identity-app | ADR (anti-gaming) | L | W3 |

## D. Builders and operators

| id | What, for whom | Builds on | Gate | Size | Wave |
|---|---|---|---|---|---|
| sdks-1.0 | Stable Rust, TypeScript, Python and Go SDKs with versioned releases | `sdks/`, `bins/l1-wallet` | publishing approval (Eric) | M | W1 |
| dev-portal | Docs, tutorials, playground and contract templates | `apps/dev-hub`, `apps/portal`, `docs/ecosystem/COURSE.md` | none | M | W1 |
| public-api | Indexer API (REST, later GraphQL) for wallets and dashboards | explorer indexer, `crates/api-gateway` | per-IP rate limits, hosting cost | M | W1 |
| mcp-server | AI assistants can read the chain and draft (never sign) transactions | `bins/maya2c-mcp` | key-safety review | S | W1 |
| node-in-a-box | One-click validator and node installer with health checks | one-line-node-join, `apps/status` | none | M | W1 |
| validator-console | Validator dashboard: uptime, rewards, slashing risk, alerts | `apps/sentinel`, staking RPC | none | M | W2 |
| pool-dashboard | Pool operators' and miners' web view | `bins/pool-service` dashboard | B34 to B37 pool items | M | W2 |
| contract-studio | Write, test and formally check WASM contracts | `crates/vm`, `crates/contract-safety`, `crates/multivm` | none | L | W3 |
| testnet-kit | Faucet, local devnet and attack lab for developers | `apps/faucet`, `cargo xtask localnet`, attacknet | none | S | W1 |
| finality-latency-bench | Per-transaction submit-to-finalized latency, with the mix, hardware and command stated; closes the measured-finality gap ([COMPETITORS.md](COMPETITORS.md) section 4) | `cargo xtask bench-tps`, `reports/bench/` | none (non-consensus) | S | W0 |
| validator-recruitment | Public validator onboarding and operator guide, so the validator count can grow beyond the disclosed launch set | `apps/status`, `apps/sentinel`, one-line-node-join | owner decision: no third-party validators before mainnet (2026-10-28 directive) | M | W1 |
| independent-audit-program | Scoped external audit of the consensus, crypto and ledger crates, with a public report | `features.toml` `kani-model-checking`, `crates/build-guard` | Eric approval and budget | L | W1 |
| succinct-state-proofs | Constant-size proofs of chain state for light clients, beyond today's SPV | `crates/stateless-core`, `crates/light-client` | research and an ADR before code | L | W4 |

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
| sentinel-ai | Watchdog AI that proposes and never acts (ADR-077) | `apps/sentinel` | ADR-077 phases | L | W3 |
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
| A council that sunsets by itself | invariant 34 (ADR-082) |

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
