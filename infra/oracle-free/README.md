# infra/oracle-free — a testnet seed at $0

One Maya2C seed validator on Oracle Cloud's **Always Free** tier: an Ampere
A1 VM (4 OCPU, 24 GB, aarch64), with the network in front of it. Why this
and not `infra/terraform/` (twelve AWS nodes, ~$9,000/month): ADR-032.

**Nothing here has been applied.** `terraform validate` passes, the rendered
cloud-init passes `cloud-init schema`, its script passes `shellcheck`, and the
pinned-commit fetch was run against GitHub. No OCI resource has ever been
created from it. The first `apply` is also its first real test.

## What it builds, and what it does not

| Layer | Who does it |
|---|---|
| VCN, subnet, internet gateway, security list, one VM | this Terraform |
| Clone the repository at a pinned commit | `cloud-init.yaml` |
| Build the node, generate genesis, validator key and wallet, firewall, systemd, Caddy | `infra/testnet-vm/install.sh` — the same installer a home PC uses to join |

Ports the internet reaches: **31100** (P2P), **4001** (chat relay), and the
API gateway — 80/443 behind Caddy when `domain` is set, 8080 otherwise. SSH
(22) only from `operator_cidr`. Node RPC stays on 127.0.0.1.

## Costs, and how not to incur one

Always Free is $0 **only inside its limits**: 4 Ampere OCPU and 24 GB in total
per tenancy, 200 GB of block storage, 10 TB/month egress. `variables.tf`
refuses values above the compute and storage limits. Two ways people get
billed anyway:

- An account upgraded to "Pay As You Go" bills for anything outside the free
  shapes. Staying on the Free Tier account type means the console refuses it
  instead. Stay on Free Tier until you mean otherwise.
- Ampere capacity is often exhausted in popular regions ("Out of host
  capacity"). Retrying later is free; switching to a paid shape is not.

## Steps

Each step marked **APPROVED** touches a live account and needs the owner's
`APPROVED: <step>` first (CLAUDE.md, Standing Order 6).

1. **Account (owner).** Sign up at oracle.com/cloud/free. It asks for a card
   for identity verification. Pick the home region carefully: Always Free
   Ampere exists only there, and it cannot be changed later.
2. **API key (owner).** Install the OCI CLI and run `oci setup config`. The
   key goes in `~/.oci/config`, never in this directory.
3. **Variables.** `cp example.tfvars terraform.tfvars` (gitignored) and fill it
   in. `git_commit` is a full SHA from `master`, after
   `infra/testnet-vm/install.sh` has landed there.
4. **Plan.** `terraform init && terraform plan` — read-only.
5. **Apply — `APPROVED: testnet seed`.** `terraform apply`. It prints
   `seed_public_ip`.
6. **Watch the build.** `ssh ubuntu@<ip> sudo tail -f /var/log/maya2c-provision.log`.
   The first build compiles the whole node on four Ampere cores; expect tens
   of minutes. The x86 release build took 12m14s on two jobs, and nobody has
   timed it on aarch64 yet.
7. **DNS — `APPROVED: seed DNS`.** In Cloudflare, add `seed1.maya2c.dev` → A →
   `seed_public_ip`, **DNS only**. Add `rpc.maya2c.dev` too if you set `domain`.

## Honest limits of one seed

- **One validator is the whole committee.** If this VM stops, the chain stops
  until it is back. Tolerating one fault needs four validators on independent
  machines (quorum 3 of 4). A second validator alone makes it worse: with two,
  either one going down halts the chain.
- **A home PC should join as an observer/full node**, not as a validator,
  until there are four independent validators.
- **One provider.** Oracle can reclaim an Always Free instance it judges idle,
  and suspend an account. The chain's state lives on this boot volume. Back up
  `/etc/maya2c` (genesis, validator key) off the VM, encrypted.

## Tearing it down

`terraform destroy` deletes the VM **and the validator key on it**. On a
testnet that is recoverable only by starting a new genesis. Copy
`/etc/maya2c` off the VM first.
