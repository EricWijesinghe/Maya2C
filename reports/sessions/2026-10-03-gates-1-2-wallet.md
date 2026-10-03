# Session 2026-10-03 — gates 1 and 2, Wasmtime, wallet redesign, testnet watchdog

Machine: Core Ultra 9 275HX, 31.4 GB, Windows 11 build 29671,
rustc 1.99.0-nightly (2026-07-14 toolchain pin).

## Testnet
- Found: chain healthy (restart at 14:10 resumed at height 208,909, no errors
  since the 2026-09-29 re-genesis), but gateway, faucet, chat relay and the
  Cloudflare tunnel were down; tunnel.log ends 2026-10-02T18:47:46Z
  "no more connections active and exiting".
- Fixed: scheduled task "Maya2C testnet watchdog" (every 5 min,
  `conhost --headless start-testnet.cmd`). Verified: killed maya-chat.exe,
  ran the task, `2026-10-03T17:29:08 maya-chat started`, Last Result 0.

## PRs
| PR | What | Local evidence |
|---|---|---|
| #42 | Wasmtime 48.0.4 (RUSTSEC-2026-0325/0326/0327) | `cargo deny check advisories`: advisories ok; maya-vm 85 passed |
| #43 | ADR-036 chain-bound signatures, gate 1 | `cargo nextest run --workspace`: 3046 passed, 0 failed, 11 skipped; spec-coverage 0 gaps; TS verifier 0 mismatches; Ledger host tests pass |
| #44 | ADR-037 mainnet with shielded pool off, gate 2 | 3056 passed, 0 failed, 11 skipped; ceremony→node end to end (started; refused with pool on) |
| #45 | Wallet redesign | desktop e2e on release build: 1 passed (6.02 s); 159 passed in affected crates |

Reviews: rust-reviewer and security-reviewer on #43 (one HIGH fixed),
blockchain-security-auditor on #44 (fork-tool config and case-insensitive
guard fixed). NOT VERIFIED: Ledger device binary (no Ledger SDK target).

## Not done
- Re-genesis of maya-testnet-1 with the new binaries: waits for #42–#45 and
  #32 (BFT halt fix) to merge.
- Gate 5: the ceremony builds only proof-of-work geneses (`bft: None`).
- Gate 4 (validators) and gate 6 (economics, symbol/decimals): Eric.
