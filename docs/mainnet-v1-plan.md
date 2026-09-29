# Mainnet v1: the plan and who does what

**Decided by Eric, 2026-09-30:** mainnet v1 launches **without private
(shielded) transfers**. The shielded pool stays off at genesis and is turned
on later at a scheduled height, after an external audit of its circuit. This
is what lets mainnet come before the audit money does: the unaudited circuit
is the reason mainnet is blocked (`docs/mainnet-readiness.md`), and a feature
that is switched off cannot mint anything.

## Launch gates

Mainnet genesis happens only when every gate below links evidence.

| # | Gate | Status (2026-09-30) |
|---|---|---|
| 1 | **Replay protection**: a transaction's signature commits to its chain, so a testnet transfer cannot be replayed on mainnet (BACKLOG P1) | open |
| 2 | **Shielded pool off at mainnet genesis**, and the mainnet guards relaxed only for that configuration (ADR needed) | open |
| 3 | **7 days on the testnet without a halt** (maya-testnet-1 halted once, at 12,530; fixed by ADR-035 and restarted 2026-09-29) | running |
| 4 | **At least 4 validators** on separate machines, run by separate people, joined through staking registration | open: needs people (Eric) |
| 5 | **Genesis ceremony** with several participants (`bins/genesis-ceremony`), rehearsed on a testnet first | open |
| 6 | **Economics**: emission stays nil, or a reward schedule, decided | open: Eric's decision |
| 7 | CI green, the security reviews of gates 1 and 2 clean, and the release built from a tag | open |

## Three lanes, three sessions

One git worktree per session. Stay inside your lane's paths, and message the
owner before editing theirs. Take turns on heavy builds (Tauri and release
builds): this machine runs out of memory with two at once.

### Lane A: protocol and launch (session `maya2c-c4`)
Paths: `crates/node` (consensus, genesis, transactions), `crates/vm`,
`crates/api-gateway`, `crates/spec-ref`, `spec/`, `infra/oracle-free`,
`apps/wallet-gui/core`, `.github/workflows/release-binaries.yml`, and the live
testnet in `D:\Maya2C-testnet` (tunnel and DNS included).
1. Gate 1: chain-bound transaction signatures for every suite (hybrid,
   suite-tagged, multisig), with spec, vectors, ADR and security review.
2. Gate 2: mainnet with the shielded pool off; the guards allow a
   value-bearing chain id only in that configuration; ADR.
3. Gate 5: a ceremony runbook, and a dry run on a throwaway testnet.
4. Gate 3: keep the testnet up. Monitoring, alerts, encrypted off-machine
   backups of the validator key, and moving the seed to Oracle Free once Eric
   has an account.
5. Releases: tag `node-v0.1.0-testnet.1` when #31 and #32 are merged.

### Lane B: chat, explorer, website (session `maya2c-b7`)
Paths: `apps/chat*`, `apps/explorer`, `apps/faucet`, `docs/site` (home,
visuals, components).
1. Explorer live at `explorer.maya2c.dev` (Lane A adds the tunnel ingress).
2. Maya Chat: a first-run experience that is exciting, not merely functional.
3. The site: a live network-status band (height, blocks per minute, faucet
   status) from the public API.

### Lane C: Maya Wallet redesign (the new session)
Paths: `apps/wallet-gui/ui` (Leptos UI and styles) and
`apps/wallet-gui/src-tauri` (commands, config). **Not**
`apps/wallet-gui/core`: signing and fee logic are consensus-adjacent, so ask
Lane A before changing them.

The feedback to fix: *"the GUI is boring."* What "done" looks like:
- A home screen that feels alive: a large balance with the live testnet
  height, recent activity, and clear Send and Receive actions.
- Receive with a QR code of the payment URI and one-tap copy.
- Onboarding that explains, in plain words, what the 24 words are and why
  they matter, with the backup confirmation kept.
- Motion and polish: transitions, loading states, a success moment after a
  send. Visibly a testnet (badge and colour), so nobody mistakes it for
  mainnet.
- The site's design tokens (`docs/site/src/styles/maya.css`), so wallet,
  chat, explorer and site look like one product. Coordinate with Lane B.
- Every existing flow still works: `apps/wallet-gui/e2e` must pass, and the
  wallet must still send on maya-testnet-1 (fees are priced by the node and
  paid to the verified collector).

### Eric
- Recruit three validator operators (gate 4): people with a machine that stays
  on. The installer is `infra/testnet-vm/install.sh`.
- The economics decision (gate 6).
- An Oracle Cloud account, so the seed leaves this PC.
- Tell the investor what they will see: the dates come from the gates above,
  not from a promise.
