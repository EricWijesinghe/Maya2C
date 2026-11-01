# ADR-077: Sentinel Recovery Network: scoped quarantine and forward-only governed recovery

- **Status:** Proposed, 2026-11-02 (Eric asked for the upgrade and for it to be documented everywhere). Seven decisions are open (D1–D7 below).
- **Full design:** [MIP-0001](../mip/MIP-0001-sentinel-recovery-network.md)
- **Prior art:** [chain-level-exploit-recovery.md](../prior-art/chain-level-exploit-recovery.md), searched 2026-11-02
- **Scope:** after mainnet (ADR-016, ADR-069). Nothing enters the launch binary. Every consensus part ships at `SRN_ACTIVATION_HEIGHT = u64::MAX`.

## Context

Chains lose large sums to bridge forgeries, contract bugs, key theft and oracle
manipulation. When they respond at all, they improvise:
- BNB Chain's validators halted the chain (Oct 2022);
- Sui's validators froze funds and then returned them after a vote (May 2025);
- Berachain halted and forked (Nov 2025);
- Flow proposed a rollback and withdrew it after backlash (Dec 2025).

The power to intervene exists on every chain whose validators can coordinate.
What is missing is rules written before the incident.

This tree already has the pieces for containment:
- the invariant guard and per-module circuit breaker (invariant 28, verified);
- the bounded 2-of-3 council pause (ADR-050);
- the threat-intel evidence model;
- guardian recovery (ADR-056).

## Decision (proposed)

Build the Sentinel Recovery Network after mainnet, as specified in MIP-0001:

1. **Sentinels are advisory.** Off-chain detectors, including ML, publish
   signed alerts. Only deterministic integer rules (invariant 20) and stake
   signatures change on-chain state.
2. **Threat levels T1–T5.** Deterministic rules raise a cell to T3 at most. T4
   needs the council plus ⅓ stake (or ½ stake). T5 (network safe mode) needs ⅔
   stake. Every level expires on its own.
3. **The Security DMZ.** Modules, contracts, bridges and registered exchanges
   are cells behind one settlement gateway. A quarantined cell refuses new
   deposits and pays out only refunds to original depositors. The core ledger
   is never quarantined, and native transfers below a threshold never stop.
4. **Forward-only recovery.** No finalized block is reverted. A passed recovery
   moves only **tainted** balances into a Recovery Escrow at a future height.
   Taint is deterministic and capped, and victims claim against
   anchor-height proofs. Nothing is minted.
5. **Bicameral, adaptive vote.** Validators approve at ⅔ stake, rising to 90%
   with impact. Holders with pre-incident balances can veto with ⅓ of votes
   cast. A challenge window and a timelock apply.
6. **Independence floor.** Recovery stays off while the committee's Nakamoto
   coefficient is below `N_min` or any operator holds more than 20%. At launch
   (ADR-047, one PC) it is therefore off by construction.
7. **Bonds, not fines.** Organizations are penalised only through a Sentinel
   Bond they post for "covered" status. Sentinels and proposers bond against
   griefing.

## Open decisions (Eric)

| # | Decision | Recommendation |
|---|---|---|
| D1 | Amend ADR-050 ("no freeze of balances") to allow freezing **tainted** balances under the bicameral vote only | yes |
| D2 | Forward-only restitution, never rollback | yes |
| D3 | Holder veto as a second house | yes |
| D4 | Recovery off until the independence floor | yes |
| D5 | Fee share for the Sentinel Fund | small; an economics decision |
| D6 | Bonds as the only penalty on organizations | yes |
| D7 | Order in the post-mainnet program | after Scam Shield (ADR-069 items 1–2) |

## Consequences

- **Good:**
  - Containment becomes automatic and scoped.
  - Recovery becomes rule-bound and public.
  - The power validators already have informally is limited in writing.
- **Bad:**
  - New governance attack surface: a captured supermajority with a
    manufactured incident is the worst case, bounded by the taint caps, the
    holder veto and challenges.
  - More consensus code.
  - No help for value already bridged out.
- **Neutral:**
  - ADR-050's council power is unchanged.
  - The invariant guard becomes SRN's T2/T3 engine for native modules.
- Every number (thresholds, windows, bonds) is a placeholder until attacknet
  scenarios S1–S8 measure them.
