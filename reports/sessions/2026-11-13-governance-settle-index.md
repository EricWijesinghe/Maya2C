# 2026-11-13: governance settlement pass reads only due proposals

Re-freeze bundle, PR #500 (base head 836b56fe). Decision: ADR-082 amendment
2026-11-13; DECISIONS.md, same date.

## Finding

`StateDB::settle_governance` (`crates/node/src/state/governance_exec.rs`)
read every `g:prop:` record on every block. Records are never deleted once
settled, so the set was every proposal ever opened, not the open ones.

## Measured cost of the old pass

Machine: Intel Core Ultra 9 275HX, Windows 11 Pro Insider Preview 29680,
shared with other builds. Profile: `test` (node crate at opt-level 0,
dependencies at opt-level 3, `[profile.dev.package."*"]`). These are
debug-crate figures. A release build was not measured.

Command:

```
cargo test -p custom-l1-node --lib governance_settle_tests -- --ignored --nocapture
```

`full-scan` is the old pass, kept verbatim as
`settle_governance_full_scan` (`#[cfg(test)]`). `indexed` is the new one.
Median of 5. None of the open proposals is due at the measured heights.

```
governance-settle open=     0 not due: full-scan median=3.9µs indexed median=2.4µs
governance-settle open=  1000 not due: full-scan median=4.0498ms indexed median=6.9µs
governance-settle open= 10000 not due: full-scan median=28.9063ms indexed median=10.5µs
governance-settle open=100000 not due: full-scan median=286.3573ms indexed median=3.6µs
governance-settle due=64 all executing: indexed median=1.7288ms
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 545 filtered out; finished in 12.80s
```

The old pass cost about 2.9 µs per stored record, and it grows linearly. A
million records were not measured. At that rate they would take about 2.9 s a
block, more than the 1 s block time. The last line is the new worst case: a
full height of 64 due proposals, every one executing a parameter change.

## Cheapest attacker cost under the old pass

- **Per-account limit:** none. **Rate:** 32 governance transactions a block
  (`MAX_GOVERNANCE_TX_PER_BLOCK`), so 32 proposals a block.
- **Deposit:** 10 coins (`GovernanceProposalDeposit` default). It is
  refundable whatever the outcome, and immediately on `CancelProposal`. It
  costs capital, not coin.
- **Fee:** base fee per byte × wire size. Measured sizes:
  `governance tx bytes: propose=13201 cancel=13208`
  (`cargo test -p custom-l1-node --test governance_due_tests -- --include-ignored --nocapture`).
- At the compiled floor (`MIN_BASE_FEE_FLOOR` = 1 base unit a byte, 10^24 a
  coin), 100,000 proposals cost 1.3201e-15 coins in fees. At the compiled
  ceiling (`MAX_BASE_FEE_PER_BYTE`) they cost 9,989 coins. The genesis
  config's floor and target set where between those it lands.
- Time: 100,000 proposals take 3,125 blocks (about 52 min), and 1,000,000
  take 31,250 blocks (about 8.7 h). Settled records stayed in the scan, so
  the attacker needed no capital beyond one block's deposits.

## Fix

The settlement index `g:due:<height BE><id>`: two keys per proposal, at most
`MAX_PROPOSALS_DUE_PER_HEIGHT` = 64 per height. The pass reads keys at or
below its height and settles those proposals in identifier order. Design and
rejected alternatives: ADR-082 amendment.

## Tests (real output)

```
cargo test -p custom-l1-node --lib governance_settle_tests
test state::governance_settle_tests::the_pass_reads_no_proposal_that_is_not_due ... ok
test state::governance_settle_tests::the_indexed_pass_matches_the_full_scan ... ok
test result: ok. 2 passed; 0 failed; 1 ignored

cargo test -p custom-l1-node --lib
test result: ok. 544 passed; 0 failed; 2 ignored

cargo test -p custom-l1-node --test governance_due_tests --test governance_tests \
  --test upgrade_schedule_tests --test genesis_tests --test payload_tests \
  --test council_pause_tests --test upgrade_rehearsal -- --include-ignored
council_pause_tests  ok. 5 passed
genesis_tests        ok. 23 passed
governance_due_tests ok. 2 passed
governance_tests     ok. 25 passed
payload_tests        ok. 29 passed
upgrade_rehearsal    ok. 4 passed
upgrade_schedule_tests ok. 7 passed

cargo test -p maya-governance
ok. 29 passed / ok. 10 passed / ok. 2 passed
```

`the_pass_reads_no_proposal_that_is_not_due` fails against the old pass. It
asserts that `settle_governance_full_scan` errors on an undecodable record
that is not due, and that the indexed pass does not read it.

## NOT VERIFIED

- Release-profile timings.
- Workspace-wide test and coverage runs, left to CI.
- `cargo xtask spec-coverage`: see the PR. No vector populates the
  governance layer, so none changed.
