# 2026-11-01: hardening wave, re-freeze, proving start

Two sessions worked in parallel (maya2c-2b and maya2c-77).

## Merged
- #268: wasmtime 48.0.8, for RUSTSEC-2026-0422/0423/0424.
- #270: CLI wallet fee query fails closed; max fee 10^24.
- #271: wallet confirm screen bound to the signing hash.
- #273: vault headroom bypass closed.
- #274: ADR-071 (to be renumbered ADR-076) base-fee clamp and ceiling, option B, 0.1 MAYA.
- #275: Hub miner path guard.
- #276: ADR-072 5 MiB block cap with first-fit selection.
- #279: DEX 1e24 price scale and supply cap.
- #287: wallet one-shot review slot.
- #289: DEX zero-price DoS. Kani `the_locked_minimum_is_never_redeemable`: VERIFICATION SUCCESSFUL, 0/420 failed, cover satisfied. Second harness re-running.
- #272: one 24-decimal COIN.
- #278: CON-13 decode bound with per-author fair share.

## Freeze
`ec8d0be1d6b57dfdb94c04abe2a0819a57db389f` (merge of #278).

## Measured
Apply time: 55.2 ms/MiB serial, 22.9 ms/MiB with parallel verify; 414 ms worst case at the 7.5 MiB decode budget. Raw output: `reports/bench/2026-11-01-apply-per-mib.md` (#296).

## Fixed on the way
- #276/#278: fee-market grep gate (`fee_market_tests`). The cap is now named through `state::fees`.
- #270: clippy collapsible_if.
- #278: clippy cloned_ref_to_slice_refs.
- apply bench: StateDB is now bound to its chain.
- CodeQL "hard-coded nonce" threads on test fixtures blocked merges under the ruleset; they were checked and resolved.

## Running
`cargo xtask proving --hours 120` in `D:/Maya2C-wt-audit` at the freeze sha. Hourly lines go to `reports/proving/`.

## Next
1. Proving passes.
2. Attacknet on the freeze sha, Hub E2E, and a self pre-audit pass.
3. Release dry run (post-proving checklist step 7).
4. Ask Eric for `APPROVED: sign genesis`.
