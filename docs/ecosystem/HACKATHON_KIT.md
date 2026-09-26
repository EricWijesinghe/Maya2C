# Hackathon kit: ready for approval

**Status: DRAFT, waiting for `APPROVED: hackathon`.** Nothing is scheduled,
announced or funded.

## Starter repos

Each one exists in this tree and has tests that pass:

| Track | Start from | Runs with |
|---|---|---|
| Payments | `crates/reference-apps/src/payments.rs` | `cargo test -p maya-reference-apps --test apps payments` |
| DeFi front end | `crates/reference-apps/src/dex_front.rs` | `… dex_front` |
| DAO tooling | `crates/reference-apps/src/dao.rs` | `… dao` |
| Security / vaults | `crates/reference-apps/src/vault.rs` | `… vault` |
| Contracts | `contracts/token-swap` (pool, no ownership) | `cargo test -p maya-vm --test token_swap_tests` |
| Wallet UX | `crates/design-system`, `design/specimen.html` | `cargo test -p maya-design-system` |

**Contracts track warning.** An ownership contract cannot be made secure until
ADR-026 lands (`contracts/nft-game` shows why). Either hold the contracts track
until then, or make "design the capability model" the challenge itself.

## Testnet faucet capacity: blocking finding

`apps/faucet` grants once per IP **and** once per address per 24 h
(`limit.rs`, `WINDOW`), and tracks up to 262,144 entries.

At a venue, every participant usually shares one NAT address. **The venue gets
one grant per day.** Before an in-person event, one of these must happen:

- a per-event allowlist of addresses, keyed by a code handed out at
  registration (a faucet change);
- pre-funded participant accounts at genesis of a hackathon devnet
  (`deploy-production.sh --target local-docker` runs 12 nodes locally,
  `reports/10-launch.md`).

The load test shows the daily cap holds under 1,000 concurrent requests, so
throughput is not the limit.

## Judging criteria (100 points)

| Criterion | Points | What earns them |
|---|---|---|
| Works | 30 | A demo runs, or tests pass, from a clean clone |
| Uses what is distinctive | 20 | PQ accounts, session keys, vault delays, clear signing, viewing keys |
| Safety | 20 | No blind-signing path, errors recoverable (the design-system flow rules), no dead ends |
| Honesty | 15 | The README says what is simulated or unfinished. Claims pass the prior-art rule (`docs/prior-art/`) |
| Usefulness | 15 | A named user with a named problem |

## Mentor guide

- **Before the event,** run every starter command above on a clean machine.
  The disk and job-count hazards are in CLAUDE.md "Build & Test".
- **The first hour.** The usual blockers are: WASM target missing (`rustup
  target add wasm32-unknown-unknown`), amounts in base units, and no
  `msg.sender`.
- **Say "not built" plainly.** Do not paper over a gap. The PLANNED items in
  `docs/architecture-vision.md` are not features.
- **Submissions** state what they measured. A demo claiming a TPS figure must
  show the command that produced it.
