# Genesis: rehearsal, launch sequence, and the first 90 days

Master Prompt 20 §4-5. **Nothing here has been run for real.** The real
ceremony, the launch and every announcement each need an explicit
"APPROVED: <step>". The tool refuses a value-bearing chain id today anyway,
because the shielded-pool circuit is unaudited (`bins/genesis-ceremony`).

## Ceremony rehearsal (at least twice, on staging, with the real participants)

1. **Freeze the inputs.** A signed parameter file: chain id, timestamp,
   difficulty, allocations, treasury share, protocol-upgrade schedule
   (empty at launch), genesis validators with their signer public keys.
   Every participant signs the file's BLAKE3 digest.
2. **Run** `genesis-ceremony` with those inputs on an offline machine. It
   generates the root keys, funds the treasury and writes `genesis.json`
   and the commitment sheet.
3. **Independent reproduction.** At least three participants recompute the
   genesis state root and block id from `genesis.json` on their own
   machines and compare with the commitment sheet. Any difference stops the
   ceremony.
4. **Validator readiness.** Each genesis validator shows:
   - its remote signer running with slashing protection enabled from the
     first block (`crates/signer`, an empty history);
   - a pinned channel to its node;
   - `maya2c-node --genesis genesis.json` reporting the agreed genesis id.
5. **Record** timings, every deviation, and the signatures. A rehearsal that
   deviated is repeated.

## Launch sequence

| T | Step | Role | Channel |
|---|---|---|---|
| T−7 d | Parameter file frozen and signed; binaries tagged and reproducibly built on two machines | launch director, release engineer | private validator channel |
| T−2 d | Final rehearsal on staging | all genesis validators | private channel + call |
| T−1 h | Validators start nodes with the agreed genesis; signers online | validators | call |
| T | Genesis timestamp passes; first blocks | — | status page |
| T+10 min | Check: finality reached, at least ⅔ of genesis stake participating | launch director | call |
| T+1 h | Public announcement, only if every check passed and "APPROVED: announce" is given | comms | public |

**Abort criteria.**

| Condition | Action |
|---|---|
| Finality not reached within 10 minutes | coordinated restart from genesis (`docs/runbooks/coordinated-restart.md`) |
| Genesis ids disagree between validators | stop, recompute, no restart until resolved |
| Participation below ⅔ at T+30 min | pause and call the missing validators; no public announcement |

## The first 90 days

- **War room**: 24/7 on-call for the first 2 weeks, a daily health report
  (SLOs, participation, incidents), and validator calls twice a week.
- **Conservative parameters**: the fee market's block target at its
  testing value (1 MiB), deferred modules off (ADR-016 activation heights
  `u64::MAX`), the validator cap of 100 (ADR-021). Governance raises limits
  only after a measured capacity review.
- **Hotfix process**:
  1. a private patch to validators over the private channel;
  2. a coordinated upgrade at an activation height (a node without the patch
     halts cleanly: "upgrade required before height H");
  3. public disclosure after the network is safe (`SECURITY.md`).
- **Reviews** at days 7, 30 and 90, comparing each SLO and the economic
  model's predictions (`reports/18-economics.md`) with what happened, and
  publishing the deviations.
