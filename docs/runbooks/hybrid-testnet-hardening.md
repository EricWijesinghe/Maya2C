# Runbook: Hardening the Oracle + PC testnet (pre-audit checklist)

**Status.** Written 2026-10-25 from the self-run threat model
(`reports/security/2026-10-25-hybrid-topology-threat-model.md`). Nothing here has
been done; this is a list with a read-only check per item. Self-run and pre-audit,
not an external assessment. Anything marked **APPROVED** touches a live server, the
Cloudflare account or keys and needs Eric's `APPROVED: <step>`.

**Why it matters.** The Oracle validator holds 71.4% of stake and alone meets
quorum, so whoever controls that VM can sign any finality. Its protection is the
chain's protection until the stake is spread.

## 1. Oracle VM
| Check (read-only) | Want | Change |
|---|---|---|
| `sudo sshd -T \| grep -Ei 'passwordauthentication|permitrootlogin|pubkeyauthentication'` | `no`, `no`/`prohibit-password`, `yes` | **APPROVED**: edit `/etc/ssh/sshd_config.d/` |
| `grep -n . ~/.ssh/authorized_keys` options column | the PC's key limited with `restrict,port-forwarding,permitopen="127.0.0.1:8545"` | **APPROVED**: a stolen PC key then cannot open a shell |
| `sudo ss -tlnp` | only sshd and the validator's p2p 31100 public; RPC on 127.0.0.1 | **APPROVED**: close the rest in the OCI security list |
| OCI security list ingress for 31100 | a rate limit or known-peer allowlist where addresses are stable | **APPROVED**: console change; mitigates floods that halt the chain |
| `ls -l` on the passphrase file of `maya2c-validator@1` | mode 600, owner the service user; not readable by `ubuntu` | **APPROVED** |
| `unattended-upgrades --dry-run -d` | security updates automatic | **APPROVED** |

## 2. Cloudflare account (public route to the PC services)
| Check | Want |
|---|---|
| Account sign-in and the GitHub account | hardware-key 2FA, no SMS fallback |
| API tokens | scoped to one zone, short expiry; none with `Account:Edit` left lying around |
| Audit log | an alert (email/webhook) on token creation and DNS/tunnel changes |
| DNS | DNSSEC on; CAA limiting issuers; SPF, DKIM, DMARC for the sending domain |
| Response headers on `*.maya2c.dev` | CSP, `X-Content-Type-Options`, `Referrer-Policy`, HSTS |
Faucet CORS trusts `maya2c.dev`, so a script injected there can call the faucet
(precedent: the 2026-10-06 injection). Changing any of this is **APPROVED**.

## 3. The PC host (validators pc1-pc4, services)
| Check | Want | Change |
|---|---|---|
| BitLocker status (`manage-bde -status`) | on for D: and E: | **APPROVED** (restart, recovery key to Eric) |
| ACLs on `E:\Maya2C-secrets`, `D:\Maya2C-validators`, `D:\Maya2C-testnet` | EricW and SYSTEM only | allowed (testnet ops), but recheck after Windows updates |
| Windows Firewall inbound | p2p ports 31401-31404 only from Oracle and known peers; 8545, 18702-4, 3100, 8080... loopback only | **APPROVED** |
| Passphrases | in DPAPI/Credential Manager, not plaintext files (`start-pc-validators.ps1` reads files today) | **APPROVED** with a script change |
| Watchdog scripts | hash recorded in git (`infra/hybrid-testnet/`); the task runs `-ExecutionPolicy Bypass`, so the directory must not be user-writable by anything else | allowed |
| `ssh` host key | pinned (`StrictHostKeyChecking=yes` with the key in `known_hosts`), not `accept-new` | allowed on the PC |

## 4. Bootstrap trust (ADR-060)
When a peer restores from a snapshot, give it `--trusted-block HEIGHT:ID` from two
paths that are not the bootstrap source, at or above the snapshot height. Without
it the source alone decides which chain the node starts on.

The same flag guards a rejoin (B49). A node whose tip is older than the unbonding window
(or whose state is more than `unbonding_epochs` epochs behind the claimed head) refuses to
start without `--trusted-block`, because validators that unbonded can sign a forged chain.
The block must be **recent**: within the unbonding window of the network's head, and above
the node's tip. Take it from the explorer or from two peers you trust, never from the peer
you are rejoining through alone.

Windows, computed as `unbonding_epochs (7) x epoch_blocks x 15 s`:

| Network (source of `epoch_blocks`) | `epoch_blocks` | Window |
|---|---|---|
| testnet (`docs/site/public/testnet/genesis.json`) | 3,600 | 105 h (4.4 days) |
| Oracle VM testnet (`infra/testnet-vm/install.sh`) | 20 | 35 min |
| localnet / devnet (`xtask/src/localnet.rs`) | 20 | 35 min |
| attacknet (`xtask/src/attacknet.rs`) | 60 | 1.75 h |
| mainnet | not in this repo; read it from the genesis file | - |

Short windows mean a node restarted after a 35-minute outage on the Oracle VM testnet or a
devnet is refused until it is given a trusted block. That is intended. Restart scripts that
pass `--catch-up-from` without `--trusted-block` (`infra/hybrid-testnet/start-pc-validators.ps1`,
`infra/testnet-vm/join-validator.ps1`, `scripts/join-testnet.sh`, `scripts/dr-drill.ps1`)
will be refused after the window. Pass the trusted block to them before the next
restart after a long outage.

A brand-new node started from a fresh genesis is not refused while the genesis block is
inside the window. A genesis older than the window (a chain that has been running longer
than that) is refused like any stale node, and needs a trusted block.

A node stale while it runs stops its follower, and the node exits so a restart can supply
the block. A chain that halts for longer than the window does this to every node.

## 5. What this does not fix
Stake concentration (a design decision: a validator above 1/3 of stake is a
single point of failure and of capture) and the absence of an external audit.
Neither is solved by hardening a host.

**Rehearsal.** not rehearsed; checklist only.
