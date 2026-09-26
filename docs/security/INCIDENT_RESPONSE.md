# Incident response

Master Prompt 16 §6. How a security incident is handled from report to
public disclosure. Names of on-call people are deliberately not in the
repository; the rota lives with whoever operates the network.

## Severity and who is paged

| Sev | Definition | Page | Response |
|---|---|---|---|
| S1 | Funds at risk, consensus split, chain halted, key compromise | security lead + two core engineers + release manager, immediately | war room within 30 min |
| S2 | Remote crash of nodes, finality degraded, exploitable DoS | security lead + one engineer | within 4 h |
| S3 | Limited-impact bug, no active exploitation | owner of the component | next working day |
| S4 | Hardening | backlog | — |

## Channels

- **Inbound:** GitHub private security advisories (SECURITY.md). Never a
  public issue, never a chat message with details.
- **Internal:** an end-to-end encrypted channel restricted to the responders
  for this incident.
- **Validators:** a pre-registered private list of validator operators with a
  PGP or age key each, used for coordinated patches.

## Coordinated patch (S1/S2)

1. Reproduce privately; write the failing regression test first.
2. Fix on a private branch; two reviewers approve (Production Standing
   Orders). If the fix changes consensus, choose an activation height far
   enough ahead for every validator to upgrade (Master Prompt 15 §4).
3. Build the release reproducibly; sign it (Master Prompt 10 §2).
4. Send the patched release to validators through the private list, with the
   activation height and a one-paragraph description that does not reveal
   the exploit.
5. Confirm upgrade share. When ≥ 2/3 of stake runs the fix and the height has
   passed, publish the advisory, the test and the full write-up.

## Emergency pause

Where the security council's emergency pause exists (Master Prompt 9), it is
the S1 tool of last resort: it pauses the affected module, never transfers,
and expires on its own. Rehearsal timing is recorded in
`reports/16-validator-security.md`.

## Communication templates

**Validator notice (private):**
> A security fix is available in vX.Y.Z (sha256 …, signed by …). Upgrade
> before height H. Do not discuss publicly until the advisory is published.
> Details follow after activation.

**Public advisory (after the network is safe):**
> On DATE we fixed ISSUE in COMPONENT, reported by REPORTER. Impact: …
> Affected versions: … Fixed in: … No action needed if you run ≥ vX.Y.Z.

## After every S1/S2

A written post-incident review within 14 days: timeline, root cause, what
detected it, what would have detected it sooner, and the regression test.
