# Maya2C Master Prompts 21–30: Beating the Giants Where They Are Stuck

## The thesis

The large chains are not weak because their engineers are weak. They are stuck because of **legacy**. They carry billions of dollars of existing contracts, wallets and user habits, so every deep fix has to stay backward compatible, get through slow governance, and be rolled out over years. Ethereum is still moving users off seed phrases. Bitcoin has millions of coins sitting behind public keys a future quantum computer could attack. Bridges keep getting drained. Users still sign transactions they can't read.

Maya2C has no legacy. It can **start** where the giants are trying to end up. That is the realistic path to being "unprecedented": not a bigger feature list, but being the first chain where the known problems are solved from genesis, together, in one coherent design.

These ten prompts each take one weakness that current giants openly struggle with, set a measurable bar that beats the best public number available today, and require evidence. Where something already exists elsewhere, the prompt says so and asks Claude Code to go further, not to pretend it's new.

**Honest limits.** No design can be guaranteed to beat every current and future chain; the giants have huge liquidity, thousands of developers, and they copy good ideas. A lasting lead comes from shipping first, proving it publicly, and giving developers a reason to stay. Those are what this set is built to produce.

**Run order:** after Master Prompt 11 at minimum (ideally after 11–16). Suggested order: 21 → 22, 23, 24 (parallel) → 25, 26, 27 → 28 → 29 → 30.

---

## MASTER PROMPT 21: Weakness Map, Prior-Art Check and the "Beat Bars"

```
/plan Act as Maya2C's Chief Strategy Engineer. Follow the CLAUDE.md Standing Orders and Production Standing Orders. No product code in this phase. This phase decides what "better than the giants" means in measurable terms.

1. WEAKNESS MAP (docs/strategy/WEAKNESS_MAP.md)
- Research, with dated and linked public sources, the top unsolved problems of: Bitcoin, Ethereum (L1 + major L2s), Solana, Sui, Aptos, Cosmos, Polkadot, Avalanche, TON, and the leading post-quantum chains (for example QRL). Cover: scalability, fragmentation, bridge security, wallet usability, smart-contract exploits, MEV, privacy, quantum readiness, node cost, developer experience, upgrade pain.
- For each weakness: evidence (incident reports, loss figures, official roadmaps admitting it), why legacy makes it hard for that chain to fix, and which Maya2C component could fix it.

2. PRIOR-ART CHECK (new Standing Order)
- Add to CLAUDE.md: "No document, UI, or announcement may call a Maya2C feature 'first', 'only', or 'unprecedented' unless docs/prior-art/<feature>.md records a dated search showing who else does something similar and exactly how Maya2C differs."
- Create prior-art files for every headline feature planned in Master Prompts 22–30.

3. BEAT BARS (docs/strategy/BEAT_BARS.md)
- For each weakness, one measurable bar with the current best public number and its source. Examples of the form (fill with researched values):
  time for a new user to go from nothing to a first transfer; percentage of transactions a user can read in plain language before signing; cross-chain transfer trust assumptions and time; finality p99 under load; cost of running a full node; time for a new developer to deploy a tested contract; value lost to exploits per value secured.
- A bar is only "beaten" when Maya2C's number is measured with the Production Standing Orders method and reproduced by someone outside the project.

4. COMPETITIVE BENCHMARK HARNESS
- benches/competitive/: scripts that run the same workload and UX tasks against public testnets or local devnets of at least three other chains, where their licenses allow. Record versions and dates. Never compare Maya2C's lab numbers with another chain's production numbers without saying so.

5. UPDATE THE LAUNCH SCOPE
- Revisit ADR-launch-scope: the adoption-critical pieces of Prompts 22 (accounts), 23 (safe contracts) and 24 (developer platform) should be considered for launch core, because developers judge a chain on day one. Record the decision.

DONE WHEN
WEAKNESS_MAP.md and BEAT_BARS.md exist with dated sources; prior-art files exist for every planned headline feature; the competitive harness runs against at least one other chain; ADR-launch-scope is updated; reports/21-strategy.md is complete.
```

---

## MASTER PROMPT 22: Accounts Without Pain: No Seed Phrases, No Gas Headaches, No Blind Signing

```
/plan Act as Maya2C's Head of Account Design. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
Seed phrases, gas tokens, unreadable signing prompts and unrecoverable mistakes are the main reasons ordinary people fail with crypto. Ethereum is fixing this through ERC-4337 and EIP-7702 on top of legacy accounts. Blind signing enabled the largest theft in crypto history (Bybit, February 2025, about $1.5 billion). Maya2C can make the fixed version the ONLY version, with post-quantum keys underneath.

1. NATIVE SMART ACCOUNTS (every account, from genesis)
- Each account is programmable: validation logic chooses which keys and rules authorize a transaction. No separate "externally owned account" type.
- Built-in policy modules: multi-key, spending limits per day and per recipient, allow-lists, time delays for large transfers, session keys for apps with scoped permissions and expiry.
- Validation cost is bounded and metered so accounts cannot be used to DoS validators.

2. KEYS PEOPLE CAN ACTUALLY MANAGE
- Default onboarding: an ML-DSA key generated inside the device's secure hardware or OS keychain, unlocked by device biometrics or a passkey. Note honestly: today's passkeys (WebAuthn) mostly sign with P-256, which is not post-quantum, so the passkey only unlocks the local PQ key; it never authorizes on-chain transactions by itself.
- Recovery without seed phrases: guardians (friends, devices, or institutions) with a delay and a cancel window, plus an optional encrypted backup. Seed phrases remain available for users who want them.
- Key rotation without changing address (from Master Prompt 13).

3. FEES THAT DON'T GET IN THE WAY
- Pay fees in any approved token (protocol-level conversion at an oracle price with a safety margin), or let an app sponsor fees with limits and abuse protection.
- Fee quote before signing that is guaranteed as a maximum.

4. CLEAR SIGNING: WHAT YOU SEE IS WHAT YOU SIGN
- Transaction intent standard: every transaction can carry a machine-readable description (contract ABI + metadata) that wallets render in plain language, in the user's language.
- Mandatory pre-sign simulation in the reference wallet: show exact balance changes, approvals granted, and contracts touched. Flag differences between what the app claimed and what simulation shows.
- Hardware wallets display the same rendered intent (extends Master Prompt 2 Ledger app).
- Test set: 200 real-world phishing and drainer patterns (approvals, permit-style signatures, look-alike contracts); the wallet must warn on every one.

5. REVERSIBILITY WHERE USERS ASK FOR IT
- Opt-in vault accounts: outgoing transfers above a limit wait N hours and can be cancelled by guardians. Normal accounts stay instant and final.

6. MEASURE
- Usability test with at least 20 people who have never used crypto: time and error rate for create account → receive → send → recover on a new device. Compare with the best current wallet you can test. Record in reports/22-accounts.md.

DONE WHEN
Smart accounts, policies, sponsored fees and guardian recovery pass tests; the phishing test set is 100% flagged; the usability study results are recorded against the beat bar; prior-art file updated.
```

---

## MASTER PROMPT 23: Safe-by-Default Smart Contracts

```
/plan Act as Maya2C's Smart Contract Safety Architect. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
Billions are lost every year to contract bugs: re-entrancy, bad access control, price-oracle manipulation, unchecked upgrades. Move-based chains (Sui, Aptos) improved this with resource types. Maya2C runs WASM, so it must bring those guarantees to a general VM and go further with protocol-enforced limits.

1. ASSETS AS RESOURCES
- In contract-sdk and the VM: tokens and NFTs are resources that cannot be copied or silently destroyed, only moved. The VM enforces this, not the contract author.
- Capability-based permissions: a contract can only touch another contract's assets through capabilities it was explicitly given.
- Re-entrancy is off by default; contracts must opt in explicitly per function.

2. DECLARED INVARIANTS, ENFORCED BY THE CHAIN
- Contracts can declare invariants (for example "total deposits ≥ total claims", "no single transaction moves more than X% of reserves"). The VM checks them at the end of every call and reverts on violation.
- Opt-in outflow rate limits (circuit breaker per contract, extending Master Prompt 8): large outflows over a window are delayed for review by the contract's guardians.

3. VERIFIED STANDARD LIBRARY
- Token, NFT, multisig, vault, AMM, lending, and governance templates written once, fuzzed, and with key properties proven (Kani / Lean from Master Prompt 8). New developers start from these.

4. SECURITY TOOLING IN THE DEVELOPER LOOP
- `maya2c-cli contract check`: static analysis, property-based tests, fuzzing, and invariant checks run locally before deploy. Findings shown in plain language with fix suggestions.
- Deploy flow shows a safety report (verified source, invariants declared, upgrade authority, audit status). The explorer shows the same report to users.

5. SAFE UPGRADES
- Upgradeable contracts must declare an upgrade authority and a timelock; the explorer and wallet show both. Storage-layout compatibility checker blocks unsafe upgrades.

6. PROVE IT
- Re-implement the 20 largest historical DeFi exploit patterns (with sources) against Maya2C templates. Record for each: blocked by the VM, blocked by an invariant, blocked by a rate limit, or NOT blocked (be honest). reports/23-contract-safety.md.

DONE WHEN
Resource semantics and capability checks pass VM tests; invariant and rate-limit mechanisms pass tests; the exploit replay table is complete with honest results; standard templates have proofs or listed open properties.
```

---

## MASTER PROMPT 24: The Developer Platform Developers Choose

```
/plan Act as Maya2C's Head of Developer Experience. Follow the CLAUDE.md Standing Orders and Production Standing Orders. Developers adopt the platform where they are productive fastest and debug fastest. Build that.

1. LANGUAGES
- First-class: Rust. Add TypeScript-like contracts through AssemblyScript or a comparable WASM-targeting language, and evaluate Go (TinyGo) and a Python-like option. Record the choice in an ADR with measured Wasm size and gas cost per language.
- EVM path (Solidity) via the Master Prompt 5 multi-VM, when activated.

2. INSTANT LOCAL LOOP
- `maya2c dev`: local chain starts in under 2 seconds, auto-redeploys on file save, pre-funded accounts, local explorer.
- Fork mode: run a local node from a snapshot of testnet or mainnet state at any height; lazy-load state on demand.

3. DEBUGGING NOBODY ELSE HAS
- Deterministic replay: take any transaction hash from any network and replay it locally step by step with the exact same result.
- Time-travel debugger: step forward and backward through a contract call, inspect storage, events, gas per line, and cross-contract calls. VS Code extension + CLI.
- Readable errors: every VM error maps to the source line and a plain-language explanation.

4. TESTING BUILT IN
- Test framework with unit tests, property tests, fuzzing, mainnet-fork tests, and gas snapshots in one command. Coverage report per contract.

5. AI-NATIVE PLATFORM
- MCP server (Model Context Protocol) for Maya2C: AI assistants can read chain state, compile, test, simulate and deploy to local and testnet (never mainnet without human confirmation).
- llms.txt and complete, machine-readable docs so AI coding tools write correct Maya2C code. Measure: an AI assistant completes 20 standard contract tasks; report the success rate and fix the docs until it is high.

6. ONE SDK, EVERY PLATFORM
- Consistent API across Rust, TypeScript, Python, Go, Kotlin and Swift SDKs (from Master Prompt 9), generated from one interface definition so they never drift.

7. DEVELOPERS GET PAID
- Protocol-level revenue share: a governance-set share of fees from a contract's usage goes to its registered developer address (extends Master Prompt 9; research prior art such as fee-sharing on other chains and document differences).

8. MEASURE
- Time from nothing to a tested, deployed contract on a clean machine, for a developer who has never seen Maya2C. Test with at least 10 developers. Record against the beat bar in reports/24-devx.md.

DONE WHEN
dev, fork mode, replay, and the time-travel debugger work end to end; the MCP server passes its task set; SDKs are generated from one definition; the developer study is recorded.
```

---

## MASTER PROMPT 25: Interoperability Without Trusted Bridges

```
/plan Act as Maya2C's Interoperability Architect. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
Bridges are the most attacked part of crypto (Ronin about $625M, Wormhole about $325M, Nomad about $190M), usually because a small group of keys or a single contract guards everything. Users also face fragmentation: many chains, many balances, many gas tokens. Maya2C's rule: every cross-chain message is verified by proof, never by a committee's word, and users should not have to care which chain they are on.

1. PROOF-VERIFIED CONNECTIONS
- Extend Master Prompt 6 light clients into ZK light clients: a STARK proof (post-quantum, from Master Prompt 2) that the source chain's consensus finalized a given event. Targets in priority order: Ethereum, Bitcoin (SPV + proof of work), Solana, Cosmos chains.
- Evaluate IBC v2 (Eureka) compatibility so Maya2C can talk to the Cosmos ecosystem and IBC-connected Ethereum without a custom bridge. ADR required.
- Document each connection's trust assumptions in one table (what an attacker must break). No connection may depend on a multisig of operators.

2. RISK LIMITS ON EVERY ROUTE
- Per-route rate limits and value caps that scale with how long the route has been running without incident. A bug in one connection can never drain more than its cap.

3. INTENTS AND CHAIN ABSTRACTION
- Users state what they want ("send 100 USDC to Alice", "swap X for Y at best price across chains"); solvers compete to fulfil it; settlement on Maya2C is verified by proof.
- One account, one balance view across connected chains in the reference wallet. Gas on the destination chain handled by the solver.

4. QUANTUM-SAFE MESSAGE FORMAT
- A cross-chain message standard with PQ signatures and proof references that other chains can adopt. Publish it as an open spec (spec/interop/).

5. TESTS
- Simulated attacks: forged Ethereum finality, Bitcoin reorg deeper than confirmation depth, solver that takes funds and doesn't deliver, relayer censorship. Each must fail safely with funds recoverable.
- Measure: time and cost of a Maya2C ↔ Ethereum transfer with proof verification; compare with major existing bridges.

DONE WHEN
At least the Ethereum ZK light client verifies real mainnet headers in tests; the trust-assumptions table is published; rate limits and caps pass tests; intent settlement works end to end on devnets; attack sims pass; reports/25-interop.md is complete.
```

---

## MASTER PROMPT 26: Scale Without Fragmentation

```
/plan Act as Maya2C's Scaling Architect. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
The giants scale by splitting: Ethereum into many L2s, other chains into shards or subnets. Splitting breaks composability (contracts on different pieces can't call each other atomically) and fragments liquidity. Maya2C's goal: grow capacity while keeping one global state that apps can compose over.

1. ONE STATE, MANY EXECUTORS
- Build on Master Prompt 12 parallel execution: scale execution across more cores and, where needed, across machines inside a validator (a validator as a cluster), while the chain still has one state root.
- Design and measure: how far one logical validator can scale horizontally; where network or storage becomes the limit.

2. LOCAL FEE MARKETS
- A hot app (for example a popular mint) raises fees only for transactions touching its state, not for the whole chain. Evaluate prior art (Solana's local fee markets) and document improvements.

3. RESERVED CAPACITY LANES
- Apps and payment providers can reserve a share of block capacity for predictable fees and latency (priced by auction, capped per holder so no one can buy the whole chain). Useful for payments and games.

4. ATOMIC CROSS-SHARD BUNDLES (if sharding is active)
- When state is spread across shards, a bundle of calls across shards commits atomically or not at all. Measure the latency cost versus same-shard calls and publish it.

5. PAYMENTS-GRADE BEHAVIOUR
- Targets (measure, don't assume): sub-second soft confirmation, finality within a few seconds, fees predictable to within a stated range under 10x load spikes.

6. PROVE IT UNDER STRESS
- Workloads from Master Prompt 12 plus a "viral app" scenario: one app suddenly takes 60% of demand. Other apps' fees and latency must stay within SLO.

DONE WHEN
Multi-machine validator prototype is measured; local fee markets and reserved lanes pass tests; the viral-app scenario keeps other apps within SLO; reports/26-scale.md has the numbers with hardware records.
```

---

## MASTER PROMPT 27: Privacy as a Developer Primitive, Compatible with Compliance

```
/plan Act as Maya2C's Privacy Engineering Lead. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
On most chains everything is public: salaries, balances, business payments. Privacy-first chains struggle with exchange listings and regulation. Projects like Aztec (private smart contracts) and Privacy Pools (proving funds are not from a banned set) show a middle path. Maya2C can offer it natively and post-quantum, as a tool every developer can use.

1. PRIVATE STATE IN CONTRACTS
- Contracts can have private state and private functions, executed on the user's device and proven with the Master Prompt 2 STARK system; the chain verifies proofs and stores commitments only.
- Public and private functions can call each other within defined rules. SDK support so a developer marks a field private and the tooling handles the proof.

2. SELECTIVE DISCLOSURE
- Viewing keys per account or per transaction for auditors, accountants and tax authorities.
- Association-set proofs: a user proves their funds come from an approved set (or not from a flagged set) without revealing which deposit is theirs. Evaluate Privacy Pools prior art and document differences.

3. PRIVATE PAYMENTS UX
- Shielded transfers as a normal option in the reference wallet, with clear explanations. Proving on a mid-range phone: measure time and memory; set a target and optimize toward it.

4. SAFETY
- Under-constraint and soundness checks from Master Prompt 2 applied to every privacy circuit. External cryptography audit required before activation (add to Master Prompt 20 audit scopes).

5. DOCUMENTATION
- docs/privacy/: what is hidden, from whom, and what is not (timing, amounts on public paths, network metadata). Plain language.

DONE WHEN
Private state and functions work in contracts; viewing keys and association-set proofs pass tests; mobile proving is measured; the privacy limits document exists; reports/27-privacy.md is complete.
```

---

## MASTER PROMPT 28: The Quantum-Safe Harbor

```
/plan Act as Maya2C's Quantum Readiness Lead. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
This is Maya2C's strongest natural advantage. Bitcoin and Ethereum have large amounts of value behind public keys that a future large quantum computer could attack, and their migration plans are still being debated. Maya2C has been post-quantum from genesis. Turn that into a service for the whole industry, while being careful and honest about what it can and cannot protect.

1. QUANTUM READINESS REPORTS
- A public tool that measures, for any Bitcoin or Ethereum address, whether its public key is already exposed on-chain, and explains the risk in plain language. Aggregate dashboards of exposed value on major chains (from public chain data, with method documented).

2. PQ VAULTS FOR OUTSIDE ASSETS
- Users bridge assets (through Master Prompt 25 proof-verified routes) into Maya2C vaults controlled by ML-DSA / SLH-DSA keys and smart-account policies.
- Honest limit, stated in the UI and docs: a bridged asset is only as safe as the weakest of (the source chain, the bridge route, Maya2C). If the source chain itself is broken by a quantum attacker, the bridged representation is affected too. The protection is against theft of the user's own exposed keys.

3. LONG-HORIZON CUSTODY FOR INSTITUTIONS
- Vault configurations for 20+ year holdings: hybrid ML-DSA + SLH-DSA signatures, SLH-DSA archival seals, scheduled key re-sealing (Master Prompt 2 archival crypto), and full audit trails.
- Compliance-ready reporting for custodians.

4. Q-DAY PLAYBOOK
- A documented, rehearsed emergency plan for when a cryptographically relevant quantum computer is announced: what Maya2C governance does within hours, days, and weeks; how suites are switched; how users are warned. Rehearse in sim/.

5. CRYPTO-AGILITY AS A SERVICE
- Offer the Maya2C crypto-agility engine and PQ libraries as open-source components other projects can use (with the Rust crates licensed accordingly). Adoption by others builds Maya2C's reputation as the reference implementation.

DONE WHEN
The exposure tool works on real Bitcoin and Ethereum data; PQ vaults work end to end on devnets with honest risk labels; the Q-day playbook is rehearsed in sim/; reports/28-quantum-harbor.md is complete.
```

---

## MASTER PROMPT 29: World-Class Interface: Wallet, Explorer and Developer Portal

```
/plan Act as Maya2C's Head of Product Design and Frontend Engineering. Follow the CLAUDE.md Standing Orders and Production Standing Orders. This upgrades the Master Prompt 9 interfaces. "World-class" is defined by measurements, not by looks.

1. DESIGN SYSTEM
- One design system (tokens for color, type, spacing, motion; component library) shared by wallet, explorer, portal and dev hub. Two themes: the signature "tactical command center" look and a calm default. Both meet WCAG 2.2 AA.
- Component docs and visual regression tests (screenshot diffs in CI).

2. WALLET UX (on top of Master Prompt 22 accounts)
- Flows: onboarding without seed phrase, receive, send, swap, cross-chain intent, recovery, vault settings, app connections, clear-signing review.
- Every flow has an explicit error and recovery state; no dead ends.
- Security UX: phishing warnings, address poisoning detection, look-alike token detection, approval manager to revoke permissions.

3. EXPLORER THAT NORMAL PEOPLE UNDERSTAND
- Every transaction explained in plain language ("Alice swapped 10 MAYA for 25 USDC on Pool X"), with the technical view one click away.
- Contract pages show the Master Prompt 23 safety report. Account pages show quantum-exposure status (Master Prompt 28).
- The 3D DAG view stays as an optional mode with the 2D fallback.

4. PERFORMANCE AND REACH
- Budgets: first load and interaction targets on a mid-range Android phone over a slow 4G profile; measure with Lighthouse and real devices; CI fails if budgets are exceeded.
- Localization: at least 10 languages at launch, chosen from where developers and users are; right-to-left support.
- Works offline for viewing balances and preparing transactions.

5. USER TESTING
- Moderated usability tests (at least 20 participants across experience levels and countries) for the top 5 tasks. Record success rate, time, errors, and satisfaction score. Fix and retest until beat bars from Master Prompt 21 are met or the gap is documented.

DONE WHEN
Design system and visual regression tests run in CI; all wallet flows pass e2e tests; performance budgets pass on the reference devices; usability results are recorded in reports/29-interface.md.
```

---

## MASTER PROMPT 30: The Developer Adoption Engine and Public Proof

```
/plan Act as Maya2C's Head of Ecosystem. Follow the CLAUDE.md Standing Orders and Production Standing Orders. Build the programs, tools and evidence that bring developers in and keep them. Anything that spends money, publishes, or announces needs "APPROVED: <step name>".

1. REFERENCE APPLICATIONS (open source, production quality)
- A payments app with a stablecoin, a DEX front end, an NFT/game example, a DAO, and a quantum-safe vault app. Each shows off Master Prompts 22–28 and serves as a template.

2. MIGRATION KITS
- Guides and tools for teams coming from Ethereum (Solidity via the EVM layer, then optional port to WASM), Solana (Anchor patterns to Maya2C), and Move chains. Measure time to port each reference app.

3. GRANTS, HACKATHONS, EDUCATION
- Grants program design (milestone-based, public reporting) funded by the treasury rules from Master Prompt 18.
- Hackathon kit: starter repos, judging criteria, testnet faucet capacity, mentor guides.
- A free course: from zero to a deployed Maya2C app, tested by beginners.

4. ECOSYSTEM METRICS (public dashboard)
- Monthly active developers (commits to Maya2C-related repos), contracts deployed and actively used, time to first deploy, developer retention after 30/90 days, SDK downloads, grant outcomes. Methods published.

5. PUBLIC PROOF
- A public benchmark report with full methodology, reproduced by at least one independent party before it is promoted.
- Publish the beat-bar table from Master Prompt 21 with current measured status: met, not yet met, or regressed. Keep it updated every release.
- Announcement copy may only use claims that pass the prior-art Standing Order.

DONE WHEN
Reference apps run on testnet; migration kits are tested with measured port times; the grants and hackathon kits are ready for approval; the metrics dashboard is live on testnet data; the public benchmark and beat-bar reports are written and waiting for my approval; reports/30-adoption.md is complete.
```
