# ADR-042: The launch path for a project with one operator

**Status:** Accepted (2026-10-05, Eric).

**Date:** 2026-10-05

## Context

The launch ladder (`docs/site/src/data/launch-gates.ts`, from LAUNCH.md)
assumed other people at three gates:

- **5: independent validators**, meaning four operators on separate machines.
- **6: attacknet + incentivised testnet**, meaning outsiders paid to attack it.
- **7: external audit.**

The project has one owner and no budget, and outside funding is promised
only after three months of stable mainnet ([investor condition,
2026-10-03]). Every gate that waits for other people also delays the event
that would pay for them. Eric's instruction (2026-10-05): bring mainnet
through every stage without depending on anyone else.

A gate cannot be passed by relabelling it. So each of the three is
restated as what one operator can honestly do, and the gap it leaves is
written down rather than hidden.

## Decision

### Gate 5 becomes "Foundation validator set"

Mainnet launches with **four validators run by the project on four
separate machines**: Eric's PC plus free-tier cloud VMs. Any one machine,
power cut or ISP outage can fail without halting the chain (n = 4,
f = 1), so the gate's *availability* aim is met.

Free tiers limit how independent the machines can be. Oracle Always Free
runs only in the account's home region; spreading its VMs over fault
domains survives a host failure, not a region outage. Two validators in one
region means that region going down halts the chain. The target layout is
therefore one validator per provider where a free tier allows it (the PC,
Oracle, and a Google Cloud e2-micro), and the runbook says which failures
each layout survives.

What it does not give is **operator independence**: one person holds all
four keys. That is stated publicly on the site and in the genesis
announcement ("foundation-operated"), and opening the set is a
post-launch milestone. Stake-weighted committees (ADR-040) and open
registration let outside validators join without a new genesis.

### Gate 6 becomes "Attacknet (self-run) + public break-it programme"

`cargo xtask attacknet` runs **daily for four weeks** with every attack it
implements:
- crash f and crash f + 1;
- a stolen key;
- garbage on the wire;
- RPC floods;
- a long outage.

Every run writes a dated report to `reports/attacknet/`. A public
"break it" page invites anyone to attack the testnet, for **credit, never
money**. The incentivised part needs a budget and a lawyer, so it moves
after funding.

### Gate 7 becomes "Pre-audit complete; external audit applied for"

The project assembles a **pre-audit package**:
- threat model;
- fuzz corpus and nightly results;
- Kani proofs;
- invariants;
- AI-assisted security scans;
- a scoped list of consensus-critical and crypto code.

It then applies to free audit programmes for open-source software, such
as OSTIF and NLnet's NGI funds, which pay for audits by firms like
Radically Open Security. Eric submits the applications.

Mainnet launches labelled **"external audit pending"** until a report
lands. Value at risk is kept low and nobody is promised anything (fees
only, no sale).

## Consequences

- The launch ladder and its status panel show the restated gates, with
  the gap in the detail line, so the site never claims independence or
  an audit that does not exist.
- Investors' three-month clock can start at a foundation-operated
  mainnet, not at a recruited one.
- Risks accepted:
  - **Concentration:** one person holds every key, so one compromise of
    Eric's machines or accounts is a compromise of consensus. Keys stay
    on separate machines, with no shared credentials.
  - **Unaudited code securing value:** mitigated by low value, fees only,
    and the audit applications.
- Reverting is cheap. Outside validators, a paid incentivised testnet and
  a paid audit each slot back in when people or money arrive. None needs
  a new genesis.

## Alternatives rejected

- **Wait for independent operators.** The launch date then depends on
  recruiting, and recruiting depends on the launch.
- **All four validators on the PC.** One Windows update halts mainnet, and
  anyone looking would see one machine.
- **Self-review in place of an audit.** A self-review is not an audit and
  would be labelled one.
