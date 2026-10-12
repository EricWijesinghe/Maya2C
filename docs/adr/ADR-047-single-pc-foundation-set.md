# ADR-047: The foundation validator set runs on one PC, said so publicly

- Status: Accepted (Eric, 2026-10-11 "yes, PC launch"; recorded 2026-10-12)
- Supersedes: the "Gate 5 becomes Foundation validator set" section of
  [ADR-042](ADR-042-solo-launch-path.md), and its rejected alternative
  "All four validators on the PC"
- Scope: launch operations. No consensus rule changes.

## Context

ADR-042 set gate 5 as four project-run validators on **four separate
machines**: the PC plus free cloud VMs. It rejected putting all four on the
PC, because one Windows restart would halt the chain and anyone looking
would see one machine.

The cloud half did not happen. Setting up Oracle Cloud was chaos for Eric,
and there is no money for paid VMs. On 2026-10-11 Eric chose to launch from
his PC alone: GitHub keeps the code, releases, CI and site, and more
machines join after launch through the public join path (`join-testnet.sh`
/ `join-validator.ps1`, the Help-run guide).

A gate cannot be passed by relabelling it (ADR-042). So this ADR says what
the single-PC set gives, what it does not, and which host failures were
closed before the gate is called met.

## Decision

1. **The foundation set is four validators on Eric's PC.** n = 4, f = 1:
   any one validator process can stop without halting the chain. On
   maya-testnet-1 this is the seed plus v2–v4, in the committee since block
   144,000.
2. **It is said publicly, in the same words everywhere:** "All four
   foundation validators run on one machine, operated by the project.
   One machine failing halts the chain until it returns; it does not fork
   it." This goes on the launch ladder, the validators page and the genesis
   announcement. Opening the set to other machines is the first post-launch
   milestone.
3. **The host failures that are the PC's fault are closed first**
   (`infra/foundation-pc/harden-host.ps1`):
   - The watchdog starts every validator **at boot, logged in or not**.
     Before this it ran only in Eric's interactive session, so an
     unattended restart left the chain halted until someone logged in.
   - Windows Update **downloads but never installs or restarts by itself**
     (AUOptions 3, NoAutoRebootWithLoggedOnUsers 1). The one testnet halt
     not caused by code (2026-10-04, 2 h 45 min) was an update restart.
   - No sleep or hibernate on mains power.
4. **Gate 5 is met** when all of these hold, each with output in
   `reports/`:
   - all four validators vote in one committee (`get_bft_status`: committee 4,
     each with a seat, none a follower);
   - `harden-host.ps1` reports OK for the watchdog, updates and power;
   - `harden-host.ps1 -SelfTest` shows a task with the watchdog's principal
     running in Windows session 0, meaning no logon is needed;
   - the public statement in (2) is live on the site.

## What it does not give

- **Machine independence.** A power cut, ISP outage, disk failure or a
  restart halts the chain for as long as it lasts. BFT safety holds: the
  chain stops and does not fork, and it resumes when the validators restart
  (maya-testnet-1 recovered from two whole-set reboots on 2026-10-09; the
  attacknet's crash-f+1 case resumed in 8.7 s).
- **Operator independence.** One person holds all four keys, as under ADR-042.
- **Protection from Insider restarts.** This PC is on the Windows Insider
  Dev channel, and Dev builds cannot simply be left: going back to a retail
  build means reinstalling Windows. The update policy in (3) defers flights
  like any other update, but Microsoft does not promise that every Insider
  flight respects it. This is the largest residual host risk. It is
  removed by moving the PC to a retail build, or by a second machine.
- **A reboot drill on the hardened host.** A deliberate restart halts the
  chain and restarts gate 3's seven-day clock (running since 2026-10-10
  19:25 KST). The drill runs after gate 3 closes; until then the evidence
  is the 2026-10-09 recoveries plus the session-0 self-test.

## Alternatives rejected

- **Keep ADR-042's four machines.** It needs cloud accounts Eric could not
  get working and money he does not have, and it blocks the launch on that.
- **PC plus one friend's machine.** Two machines with four seats still put
  at least two seats on one machine. With 2 + 2, either machine failing
  halts the chain; with 3 + 1, the PC failing halts it. It adds a second
  operator's key without buying liveness.
- **Fewer validators (n = 1).** It loses the one thing four processes do
  give: a validator crash, a bad upgrade of one binary or a corrupted data
  directory does not halt the chain.

## Consequences

- Launch does not wait for anyone else, and what is claimed matches what
  exists.
- The status page and the announcement must never call the set
  "decentralized" or "independent" until other machines hold a third of
  the stake.
- `docs/runbooks/foundation-validators.md` keeps the four-machine layout as
  the target. It is what replaces this ADR when machines or money arrive,
  and no new genesis is needed (ADR-040: stake-weighted committees, open
  registration).
