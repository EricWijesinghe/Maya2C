# A public Maya2C testnet on one VM

One command turns a fresh Ubuntu 22.04 or 24.04 VM into a public testnet:
four DAG-BFT validators, the API gateway (with HTTPS if you have a domain)
and a Maya Chat relay. Each runs as a sandboxed systemd service.

```sh
git clone https://github.com/EricWijesinghe/Maya2C && cd Maya2C
sudo DOMAIN=testnet.example.org EMAIL=you@example.org infra/testnet-vm/install.sh
```

Without `DOMAIN`, the gateway is served as plain HTTP on port 8080. To
preview every change without making any, run it with `DRY_RUN=1`.

## Getting a VM for free

Any Ubuntu VM with 4 GB of RAM and 20 GB of free disk will do. Oracle Cloud's
Always Free tier includes an Arm VM with up to 4 cores and 24 GB of RAM at no
charge; check the offer's current terms when you sign up, because free tiers
change. In the cloud console, open inbound TCP 22, 31100, 4001, and either
80 and 443 (with a domain) or 8080 (without). The installer configures the
VM's own firewall to match.

**Standing Order 6:** creating the VM, pointing DNS at it and running the
installer is the owner's step. Say `APPROVED: public-testnet` first.

## What is exposed, and what is not

| Port | Service | Public |
|---|---|---|
| 22 | SSH | yes (restrict it to your own IP in the cloud console) |
| 31100 | validator 0 libp2p (the bootnode other operators join) | yes |
| 4001 | Maya Chat relay | yes |
| 443 / 80 or 8080 | API gateway (REST/GraphQL) | yes |
| 32000–32003 | node JSON-RPC | **no**: loopback only, behind the gateway |
| 31101–31103 | validators 1–3 libp2p | **no**: they peer over loopback |

The keys are generated on the VM itself:

- The validator keys are in `/var/lib/maya2c/v*/validator.key`, readable
  only by the `maya` user.
- The funded testnet wallet is `/etc/maya2c/wallet.key`, with a random
  password in `/etc/maya2c/wallet.password`; both are root-only.

The installer never prints any of them.

## How many validators, and where

BFT tolerates `f = floor((n - 1) / 3)` faulty validators and needs a quorum
of `n - f`. That makes the count matter more than it looks:

| Validators | Faults tolerated | What halts the chain |
|---|---|---|
| 1 | 0 | the one machine going down |
| 2 | 0 | **either** machine going down: worse than 1 |
| 3 | 0 | any one of them going down |
| 4 | 1 | two of them going down |

Four validators on one VM (`VALIDATORS=4`, the default) have finality but
still share one machine. The first real fault tolerance comes from four
validators on **four separate machines**, ideally with separate providers.
Until then, run a second machine, such as a home PC, in JOIN mode as an
observer (a full node that follows and verifies the chain). Do not register
it as a second validator.

A new validator can join after genesis: it submits a staking registration,
and it enters the committee at the next epoch boundary
(`crates/node/tests/bft_staking_tests.rs`,
`a_registration_joins_the_committee_and_evidence_removes_an_equivocator`).
A joining node prints its public key to `/etc/maya2c/v0.pub` for that
purpose.

## What this is not

- **Not decentralised.** Four validators on one machine have BFT finality,
  but a single operator. Other operators join through staking (LAUNCH.md
  Gates 2 and 3).
- **Not audited.** No external security audit has reported yet (`cargo xtask
  go-no-go`). Nothing on this chain carries value, and the installer refuses
  a chain id that names mainnet.
- **Not the remote signer.** Validators sign with local key files; the
  remote signer (`crates/signer`) is not yet called by the node (ADR-022).

## Operating it

```sh
journalctl -u 'maya2c-*' -u maya-chat-relay -f     # logs
systemctl status 'maya2c-validator@*'              # health
sudo infra/testnet-vm/install.sh                   # upgrade: rebuild, restart, keep keys and chain
```

Publish `/etc/maya2c/genesis.json` together with the sha256 the installer
prints. Anyone joining checks that hash, which is how they know they joined
this chain and not another.

## Tested on

See the end of this file: the host the installer was last run on, the
commit, and what it checked.
