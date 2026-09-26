# Zero to a deployed Maya2C app: course outline

**Status: outline, not yet tested by beginners.** The brief requires beginner
testing. No beginner has taken it, so the lesson times below are targets, not
measurements. The first cohort replaces them with real figures (the same
protocol as the usability study in `reports/29-interface.md`).

| # | Lesson | Build | Target time | Checked by |
|---|---|---|---|---|
| 1 | Accounts without seed phrases | Open a `Wallet`, send a transfer, read the receipt | 30 min | `reference-apps` payments test |
| 2 | Keys that can't overspend | A till session key: scope, limit, expiry | 45 min | the payments test's refusals |
| 3 | Reading before signing | Clear-signing review; catch a lying page | 45 min | `dex_front` review test |
| 4 | Your first contract | Build `contracts/token-swap`; run it in `Vm::execute` | 60 min | `token_swap_tests` |
| 5 | Why ownership needs the runtime | Run `nft_game.rs`; explain the exploit | 30 min | `anyone_can_move_anyones_token` |
| 6 | Governing money | A DAO grant through vote, timelock and treasury | 45 min | `dao` test |
| 7 | Vaults and recovery | Delays, guardian cancel, auditor viewing key | 45 min | `vault` test |
| 8 | Shipping a UI | Tokens, contrast, flows without dead ends | 60 min | `maya-design-system` tests |
| 9 | Deploying | Local 12-node devnet (`deploy-production.sh --target local-docker`) | 60 min | node RPC health |

**Lesson 9 is deploying to a local devnet, not a public testnet**, because
none is public. "Deployed" in the title means "deployed to a network you
started". That will change when a testnet exists.

## Beginner test protocol

- **Participants.** 5 or more who have never written Rust, and 5 or more who
  have never used a blockchain.
- **Per lesson, record:** completion without help, time, where each person got
  stuck, and a 1–7 difficulty score.
- **When to rework.** A lesson is rewritten when fewer than 4 of 5 complete
  it unaided.
