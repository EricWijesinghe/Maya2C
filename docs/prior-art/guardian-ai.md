# Prior art: Guardian AI (watches the network, proposes fixes, never acts alone)

**Search status:** searched 2026-11-10 (web search; sources cited below).

Feature: CATALOG §I row `guardian-ai` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). The design is ADR-077's advisory sentinels; the sentinel app exists, the AI tier does not. R0.

**Queries:** "Forta Hypernative onchain monitoring AI automated response pause contract 2026"; "AI agent guardian blockchain propose not execute human approval safety architecture 2026 paper".

## Who else does something similar

| Project or work | What it does | Source |
|---|---|---|
| Forta Network | Detection bots (heuristic and ML) publish alerts. Detection is off-chain and advisory. Pausing is not native: Forta's example pauses through OpenZeppelin Defender triggered by an alert. | [Forta blog](https://forta.org/blog/preventing-smart-contract-exploits-with-automatic-pausing), [Forta on AI detection](https://forta.org/blog/how-ai-based-threat-detection-can-save-web3) |
| Hypernative | AI-driven monitoring that can act: automated pause or unwind with an Emergency Admin role. The opposite of "never acts alone". Self-reported results only. | [Hypernative](https://hypernative.io/products/onchain-monitoring) |
| "Agents propose, humans sign" | Ledger's roadmap: AI prepares transactions, a hardware device must confirm. Vendor announcement. | Ledger roadmap (search snippet only; the page URL was not captured, so re-check before citing) |
| Propose-only contract pattern | Developer guide: AI holds a whitelisted propose-only address; a separate executor checks approval; a multisig guardian can veto. How-to, not research. | [ChainScore guide](https://www.chainscorelabs.com/guides/ai-llms-and-machine-learning-in-web3/ai-powered-smart-contract-development/setting-up-governance-for-ai-proposed-smart-contract-changes) |
| arXiv 2609.04017 (Sept 2026) | Position paper: blockchain-anchored commitments for agent communications and human approvals, as an audit layer. States it does not prevent agent misbehaviour. No evaluation. | [arXiv](https://arxiv.org/pdf/2609.04017) |

No single paper matched the full "AI proposes, human approves, chain executes" description in this search.

## What Maya2C has (checked in repo, 2026-11-10)

- `docs/adr/ADR-077-sentinel-recovery-network.md`, decision 1: "Sentinels are advisory. Off-chain detectors, including ML, publish signed alerts. Only deterministic integer rules (invariant 20) and stake signatures change on-chain state."
- Same ADR: "Deterministic rules raise a cell to T3 at most." T4 and T5 need council and stake votes. So a deterministic rule can raise a threat level without a human; no AI output can.
- `apps/sentinel/src/detect.rs`, `apps/sentinel/src/notify.rs`: the sentinel app (detection and notification). Its ML path is off-chain.
- Invariant 20 (`docs/invariants.md`): no float in a consensus rule.

## How a Maya2C version would differ

- **Consensus boundary.** Model output is never an input to a state transition. Forta and Hypernative alerts are also advisory or off-chain, so this is the same boundary as Forta, with the rule written into the state machine (ADR-077).
- **Narrower automatic path than Hypernative.** Hypernative can pause on its own. A Maya2C deterministic rule can raise a cell to T3 automatically, which is an automatic action, and T4 and above need votes. The claim is "AI never acts," not "nothing acts without a human."
- **Not built.** The AI tier (proposals, the propose-only interface) is not in the repo, so the guardian is not claimed as working.

**Verdict: similar exists (Forta alerts, Hypernative automated response, propose-only contract patterns). Differentiate by a consensus rule that keeps model output out of state, with deterministic automatic rules stated plainly. Not a novelty claim.**

**forbidden**: "first AI guardian", "only chain with a safe AI", "the AI can never act" (a deterministic rule can act; say "AI output never changes state").
