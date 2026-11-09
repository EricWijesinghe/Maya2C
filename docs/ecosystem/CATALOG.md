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
| hub-launch | Open Wallet and Chat from the Hub through an OS link scheme (ADR-083 decision 4) | `apps/hub`, `apps/wallet-gui`, `apps/chat-gui` | deep-link plugin review, installer registration | S | W0 |
| address-text-everywhere | Bech32m `maya1…` in the wallet GUI, explorer, pool, chat and docs (ADR-081 leftovers) | `crates/address` | none | S | W0 |
| signed-installers | Signed installers for Windows, Linux and Android for every app, with auto-update | `apps/app-update`, ADR-080 | code-signing cost (Eric) | M | W0 |
| wallet-v2 | Clear signing, smart-account recovery without a paper seed, address book, payment requests | `apps/wallet-gui/core`, `crates/smart-account`, `crates/clear-sign` | ADR for the recovery UX | L | W1 |
| wallet-mobile | Android wallet (light client); iOS after the MacBook | `crates/light-client`, wallet core | platform targets (memory 2026-10-12) | L | W2 |
| wallet-extension | Browser extension wallet for web apps | wallet core, `crates/web-security` | ADR (dApp connection protocol) | M | W2 |
| hardware-wallet | Ledger app for ML-DSA signing | `apps/ledger-maya2c` | device memory for PQ signatures, Ledger review | L | W3 |
| chat-v2 | Group chats, attachments, multi-device | `apps/chat`, ADR-074 | ADR (group key schedule, MLS) | L | W1 |
| chat-pay | Send MAYA inside a chat; the wallet signs (ECOSYSTEM.md section 3) | chat + wallet hand-off | hand-off protocol ADR | M | W1 |
| explorer-v2 | Graphs, address pages in Bech32m, validator and mining pages, screenshot-diff CI (C10) | `apps/explorer` | none | M | W1 |
| status-v2 | Public network status with a history of incidents and gate progress | `apps/status`, `apps/sentinel` | none | S | W1 |
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

Waves W1 to W3 exist to close them.

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
