---
title: Break it
description: Attack maya-testnet-1 and the Maya2C code. Credit for every real finding, no money promised. Rules, targets and how to report.
---

Maya2C is built by one person, and it will only be trusted once people
other than its author have tried hard to break it. This page invites you
to try.

:::caution[Credit, not money]
Findings are rewarded with **public credit**: your name in the advisory, in
the release notes and on this page. There is no budget for bounties, and
nothing here promises tokens, money or a mainnet allocation.
:::

## What we already attack ourselves, every day

The project runs `cargo xtask attacknet` daily: seven validators on one
machine, attacked as follows.
- Two crash; then three crash, which must **halt**, never fork.
- A stolen key signs from a second machine.
- Random bytes hit hundreds of peer connections.
- The RPC is flooded with malformed and invalid requests.
- One validator is down past the engine's window and must rejoin.

Every day's report, failures included, is published unedited:
**[attacknet reports](https://github.com/EricWijesinghe/Maya2C/tree/attacknet-reports)**.
One operator attacking their own network is a floor. This page is how it
becomes more than that.

## Targets

| Target | Where | Worth finding |
|---|---|---|
| Consensus | DAG-BFT, attested checkpoints, catch-up, stake-weighted quorums | a fork, a halt the rules do not justify, a validator that cannot rejoin |
| Signatures | ML-DSA-65 + SLH-DSA hybrid, `Transaction::verify` | a transaction accepted with one bad signature, malleability |
| State | apply path, state root, staking, slashing | value created or destroyed, a root two honest nodes disagree on |
| Network | libp2p, the ML-KEM handshake, the WebSocket bootnode | a crash or stall from crafted frames, peer-slot exhaustion with amplification |
| Public services | `rpc.maya2c.dev`, `faucet.maya2c.dev`, `bootstrap.maya2c.dev` | a method the gateway should refuse, faucet limits bypassed |

The code is at [github.com/EricWijesinghe/Maya2C](https://github.com/EricWijesinghe/Maya2C).
The fastest way in is to run the attacknet yourself and add an attack it
does not have.

## Rules

- **Testnet only.** Attack `maya-testnet-1` and your own local nodes, never
  another person's machine.
- **No volume floods** against the public endpoints. They run on one home
  connection. A crash from *one* crafted request is a finding; a crash from
  a million requests is not.
- No social engineering, no attacks on accounts (GitHub, Cloudflare,
  email), and no use of a key that is not yours.
- Stop at proof. Do not drain the faucet or spam the chain beyond what
  shows the bug.

## Reporting

Report privately with GitHub's **[Report a vulnerability](https://github.com/EricWijesinghe/Maya2C/security/advisories/new)**,
as [SECURITY.md](https://github.com/EricWijesinghe/Maya2C/blob/master/SECURITY.md)
describes. Include what you found, how to reproduce it, and the commit you
tested. You will get an acknowledgement within 3 working days. Please
allow 90 days, or until a fix ships, before publishing.

## Hall of fame

No external findings yet. The first name here is yours to take.
