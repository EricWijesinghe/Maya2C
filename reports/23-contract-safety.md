# 23 — Safe-by-default smart contracts

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 23

> DONE WHEN: resource semantics and capability checks pass VM tests;
> invariant and rate-limit mechanisms pass tests; the exploit replay table is
> complete with honest results; standard templates have proofs or listed
> open properties.

| Condition | Result |
|---|---|
| Resource and capability checks pass tests | **pass**, in `crates/contract-safety`. They are **not VM tests**: the rules are not wired into `crates/vm` |
| Invariant and rate-limit mechanisms | **pass** (rollback, queued outflows) |
| Exploit replay table, honest | **complete**: 20 patterns, 5 not blocked |
| Standard templates with proofs | **not built.** Open properties are listed below |

`crates/contract-safety` is the rule layer a VM host would call:

- move-only resources: there is no copy API, and mint and burn need
  capabilities;
- capability-gated access to another holder's assets;
- re-entrancy refused unless a function opts in;
- declared invariants checked when the outermost call ends, with complete
  rollback;
- per-contract outflow limits that queue excess for guardians.

It is dependency-free, so Kani can compile it. ADR-016 named this library as
launch core before it existed (`reports/21-strategy.md`); it now exists, as
a library.

## The replay table

```
$ cargo test -p maya-contract-safety --profile ci -- --nocapture
| The DAO | 2016-06 | re-entrancy | Vm |
| Curve / Vyper | 2023-07 | re-entrancy (broken compiler lock) | Vm |
| Poly Network | 2021-08 | privileged call without authority | Vm |
| Parity multisig | 2017-11 | library destroyed, funds frozen | Vm |
| Nomad | 2022-08 | zero root accepted: payout beyond deposits | Invariant |
| Euler | 2023-03 | donation broke the solvency check | Invariant |
| Cetus (Sui) | 2025-05 | overflow credited unbacked liquidity | Invariant |
| KyberSwap | 2023-11 | tick-math precision minted liquidity | Invariant |
| Mango Markets | 2022-10 | oracle manipulation, then borrow | RateLimit |
| Cream Finance | 2021-10 | flash-loan price manipulation | RateLimit |
| bZx | 2020-02 | flash-loan oracle, small drain | NotBlocked |
| Beanstalk | 2022-04 | flash-loan governance capture | RateLimit |
| Ronin bridge | 2022-03 | 5 of 9 validator keys | RateLimit |
| Harmony Horizon | 2022-06 | 2 of 5 multisig keys | RateLimit |
| Multichain | 2023-07 | MPC keys controlled by one person | RateLimit |
| Bybit | 2025-02 | blind-signed malicious upgrade | RateLimit |
| Wormhole | 2022-02 | signature check bypassed: unbacked mint | NotBlocked |
| BNB bridge | 2022-10 | forged proof: unbacked mint | NotBlocked |
| Badger DAO | 2021-12 | front-end injected approvals | NotBlocked |
| Wintermute | 2022-09 | vanity key brute-forced | NotBlocked |
20 patterns: {"Invariant": 4, "NotBlocked": 5, "RateLimit": 7, "Vm": 4}
test result: ok. 3 passed; 0 failed …
```

Outcomes are computed from what the runtime returns, then asserted, so a
weakened rule fails the test.

**How to read it honestly.**

- **Each row models the pattern's mechanism, not the original contract.** A
  replay shows the rule catches that *shape*. It does not show that the
  historical contract, ported, would have been safe.
- **"Invariant" only helps if the author declared the right invariant.**
  Nomad and Euler are caught by "the vault holds what it owes"; Cetus and
  KyberSwap by "shares are backed". A contract without those declarations
  gets no protection. Templates (§3 of the brief) are where the defaults
  would come from.
- **"RateLimit" means delayed, not blocked.** Seven of the largest losses
  were outflows the contract itself authorized: stolen keys, captured
  governance, a blind-signed upgrade. The runtime cannot tell them from
  legitimate withdrawals. It can only hold anything above the cap (10 %/day
  here) for guardians, who must then notice. bZx's drain was under the cap
  and went through.
- **Five are not blocked.** Wormhole and BNB minted against forged proofs
  whose backing lives on another chain; nothing local can check it without
  a light client (Master Prompt 25). Badger's victims had granted the
  allowance, and clear signing (`reports/22-accounts.md`) flags that before
  signing, but the VM allows it. Wintermute's key was the owner's key.

Loss figures are not repeated here. Sources: the rekt.news leaderboard
(https://rekt.news/leaderboard/) and the linked post-mortems in
`docs/strategy/WEAKNESS_MAP.md` for Ronin, Wormhole, Cetus and Bybit.

## Not done

- **Wiring into the WASM VM.** The host functions would call this layer on
  every value-moving import; `crates/vm` does not yet.
- **Verified standard templates.** Token, NFT, multisig, vault, AMM, lending
  and governance, with proofs. The open properties each needs:
  conservation, no-unbacked-shares, solvency, and bounded outflow.
- **`maya2c-cli contract check`**, the deploy safety report, and the
  upgrade authority/timelock/storage-layout checker.
