# Owner plan: the zero-budget path to a company

Written 2026-09-29 for Eric. Resources: one workstation (Core Ultra 9, RTX 7070,
32 GB DDR5), unlimited internet, Claude and Claude Code, and **$100**.
This plan assumes no other money arrives. It is a plan for that reality, not a
plan that waits for funding.

## 1. Where the project actually is

From the repository's own evidence, not from a brief:

- `cargo nextest run --workspace` — 2,972 passed, 0 failed (sweep 2026-09-28).
- `cargo xtask spec-coverage` — 44 rules, 71 vectors, 0 consensus gaps.
- `cargo xtask go-no-go` — **8 PASS, 5 FAIL, 3 NEEDS HUMAN: NO-GO**.
- `cargo xtask readiness` — 47 of 104 cells carry evidence; the rest are GAP.
- Milestone M1 CORE SOLID: 6 of 10 exit criteria met.
- https://maya2c.dev is live and costs nothing to run.

The engineering is further along than the money is. Every remaining go/no-go
failure is an owner item, and most of them need people or cash, not code.

## 2. What is missing, in three buckets

**A. Free — decisions only you can make (this week).**
LICENSE (none exists); what Maya Chat's honest difference is; mining vs
ADR-016 (no block reward); whether M1 needs all of MP06 or its core parts;
`bft.fees` in mainnet genesis.

**B. Free — work Claude Code can do (weeks, not money).**
Core line coverage (M1 #8); property tests for fees, consensus, VM (M1 #5);
gap-register tooling; the remaining M2/M3 measurement work; the chat app.

**C. Costs money or needs other people.**
External audits ($50k–$200k+ for a chain of this size); attacknet and
incentivized testnet (4 weeks each, needs a public network and participants);
external validators across providers and countries; code-signing certificate
and an Apple developer account; legal sign-off; a dedicated benchmark runner;
a Mac and a phone for MP17/MP27; usability study participants.

**Mainnet cannot happen on $100.** Launching a chain that holds other people's
money without an external audit is how founders lose both the money and their
reputation. That is not a rule invented here; it is the go/no-go gate already
written into this repository.

## 3. What the $100 is for

Suggested allocation, and nothing else:

| Item | Cost | Why |
|---|---|---|
| Domain renewal (maya2c.dev) | ~$15/yr | Already the project's public face |
| Reserve | ~$85 | A VPS month if a free tier is reclaimed, or a hackathon fee |

Everything else runs free:

- **Testnet nodes** — Oracle Cloud Always Free (ARM, generous), Google Cloud
  free tier, plus your own workstation behind a Cloudflare Tunnel as a seed.
  `infra/oracle-free` already exists (ADR-032); it needs an Oracle account.
- **CI, hosting, DNS, TLS** — GitHub Actions and Pages, Cloudflare: free for
  public repositories.
- **Code signing** — check SignPath Foundation, which signs open-source
  projects for free. Buy no certificate until that is ruled out.
- **Your GPU** — load generation and GPU/CPU parity tests, not mining. Mining
  your own testnet earns nothing and costs electricity.

## 4. The funding ladder (in order, no token sale)

1. **Grants for open-source infrastructure.** Post-quantum cryptography and
   private peer-to-peer messaging are exactly what several funds exist for.
   Check current open calls: NLnet / NGI Zero, Open Technology Fund's Internet
   Freedom Fund, Ethereum Foundation ESP (for post-quantum work that helps the
   wider ecosystem), and language/ecosystem foundations. Typical size €5k–€50k,
   no equity taken. This is the single most realistic first money.
2. **Hackathons with prize money** — online, free to enter, and they force a
   demo people can see.
3. **Freelance Rust or security work, part-time.** You have a 2,900-test Rust
   codebase as a portfolio. Ten to fifteen hours a week funds an audit deposit
   faster than waiting does.
4. **Sponsorship** — GitHub Sponsors and Open Collective, once the chat app has
   users.
5. **Investors** — only after a public testnet and users exist. Before that
   there is nothing to value.

**Do not** pre-sell a token to fund development. In most jurisdictions that is
a regulated securities offering, it is the fastest way to a legal problem you
cannot afford, and it would destroy the credibility this repository's honesty
rules exist to protect.

## 5. The 90-day plan

**Days 1–14 — close the free gaps.**
Answer the five decisions in §2A. Pick a licence and commit it. Finish M1:
line coverage and the missing property tests. Get one full green CI run on
master. Outcome: M1 CORE SOLID met by evidence.

**Days 15–45 — a public testnet that costs nothing.**
Oracle free-tier seed node plus your workstation. Faucet, explorer and status
page on the existing site. Publish the quickstart so a stranger can join with
one command. Outcome: a live network anyone can point a node at, and the first
outside participant.

**Days 46–75 — ship Maya Chat.**
It does not depend on the chain, so it is the fastest route to real users. One
honest claim, decided in §2A. Publish the abuse policy before launch, not after.
Outcome: an app people can install, and feedback from users who are not you.

**Days 76–90 — apply for money, in public.**
Two to three grant applications, using the testnet and the chat app as
evidence. Publish the measured numbers with the method, per the Production
Standing Orders. Outcome: applications in flight and a track record that makes
the next one easier.

Mainnet is not in the first 90 days. It sits behind audits, and audits sit
behind funding.

## 6. Weekly rhythm

- **Mon** — `cargo xtask status`; pick this week's one goal from EMPIRE.md.
- **Tue–Fri** — Claude Code sessions under the Operating Protocol.
- **Sat** — the verification sweep if one is due; update STATE.md and RISKS.md.
- **Sun** — rest. A solo founder who burns out ships nothing. Key-person risk
  is R1 in RISKS.md for a reason.

Every 5 sessions, read the executive report. If two weeks pass with no
downgrade and no new evidence, something is being claimed rather than measured.

## 7. The honest risk

One person, no money, a large codebase, and a market full of funded teams. The
realistic outcome is not "crypto giant next year". It is: a credible open-source
post-quantum chain with a working testnet and a real chat app, which attracts a
grant, then a contributor, then an audit, then a launch. That path is slow and
it is achievable. The fast path is the one that ends in a hacked mainnet.
