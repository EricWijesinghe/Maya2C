# Deployment pipeline

Zero to a twelve-node fleet. Four stages, and the first thing to know is that
the last one will refuse to run on mainnet.

## Mainnet is blocked, and this pipeline enforces it rather than working around it

The shielded pool's Groth16 parameters come from a reproducible test setup, not
a ceremony. Anyone able to re-run that setup holds the toxic waste and can mint
shielded value that no supply audit would reveal — including the supply
endpoints this repo serves publicly.

Six independent places refuse a value-bearing chain id. This pipeline adds two
more and removes none:

| Where | When it fires |
|---|---|
| `zk-privacy/src/prove.rs` | `SETUP_IS_TRUSTED = false` |
| `src/bin/node.rs` | the node exits at startup |
| `src/bin/genesis-ceremony.rs` | refuses to mint the genesis |
| `terraform/modules/node-pool/variables.tf` | `terraform validate`, before any provider call |
| `terraform/modules/gke-node-pool/variables.tf` | same, on GCP |
| `faucet/src/lib.rs` | refuses to construct |
| **`infra/ansible/setup_node.yml`** | pre-task assertion, before anything is installed |
| **`scripts/local_cluster.sh`** | first thing it checks |

Everything here is built so that the day a real ceremony completes, launching
mainnet is one flag and a genesis file — not a re-plumbing. Today it deploys
`maya-genesis-rc1`.

## The four stages

### 1. Provision — `terraform/`

Twelve nodes: four in each of `us-east-1`, `eu-central-1`, `ap-southeast-1`.
Disjoint VPC CIDRs, because nodes gossip across regions and a future peering
cannot be retrofitted onto three copies of `10.0.0.0/16`.

```bash
cd terraform
terraform init
terraform plan -var 'chain_id=maya-genesis-rc1'
```

`instance_type` defaults to `r6i.4xlarge` (128 GiB). The floor is the
proof-of-work dataset — 4 GiB resident on mainnet — sitting alongside RocksDB's
block cache and write buffers, not request volume.

`bare_metal = true` selects `r6i.metal` instead. Read the cost note in
`variables.tf` first: it is roughly $50,000 a month for twelve against $9,000
for the virtualised shape, and it buys single-tenancy, not more memory.

### 2. Ceremony — `src/bin/genesis-ceremony.rs`

**Two steps, and the coordinator ends up holding no key material.**

Each participant, on their own machine:

```bash
genesis-ceremony contribute --label alice --out-dir ./alice
```

That writes `alice.secret` (mode 0600, never printed) and `alice.public`. Only
the public half is sent anywhere.

The coordinator, over the collected public halves:

```bash
genesis-ceremony assemble \
    --chain-id maya-genesis-rc1 \
    --supply 21000000000 \
    --timestamp 1767225600 \
    --contributions ./contributions \
    --treasury-public ./treasury/treasury.public \
    --out-dir ./ceremony
```

`assemble` refuses a contributions directory containing a `.secret` file. That
is not tidiness: a secret that reached the coordinator has already left the
machine that generated it, and continuing would produce a genesis whose custody
story is quietly false. Rotate that root while rotating is still free.

The single-party form — no subcommand — still exists for testnets where one
operator holds everything anyway. It is the wrong tool for a launch, and the
reason is the one above.

**There is no "Genesis DAG root" to seal.** `crypto::dag` is the Ethash-style
memory-hard proof-of-work *dataset*, derived deterministically on each node from
its epoch seed — generated, not distributed. What genesis actually commits to is
the **state root** and the **genesis block id**, and those are what
`COMMITMENT.txt` carries and what every operator diffs.

### 3. Configure — `infra/ansible/`

```bash
ansible-playbook -i inventory/production.yml setup_node.yml \
    -e maya_expected_state_root=<from COMMITMENT.txt> \
    -e maya_genesis_url=https://...
```

`serial: 1` — a rolling change that took every seed down at once would partition
the network from itself.

The play verifies the fetched `genesis.json` against the ceremony's state root
using `genesis --verify --expect-state-root`, which **fails the play** on a
mismatch. A fleet split across two genesis files does not report an error; it
looks like a fleet that is merely slow to converge, which is why this check is
not optional and not a warning.

Systemd units are installed from `deploy/systemd/` verbatim. They are already
hardened — `DynamicUser`, `ProtectSystem=strict`, an empty
`CapabilityBoundingSet`, a `SystemCallFilter` — with the reasoning inline.

**TLS goes on the gateway, never on a node.** The node's JSON-RPC has no
authentication and serves `get_mining_candidate` and `submit_block`. A
certificate on it would publish the miner interface with a padlock beside it. So
RPC binds to loopback, UFW keeps it there, only P2P is world-reachable, and
`roles/maya_gateway` is the only thing that talks to Let's Encrypt. That role
asserts it is running on a host in `maya_gateways` and refuses otherwise.

**Building on target hosts is the default and is worth reconsidering.** It puts
a Rust toolchain on twelve production boxes and produces twelve binaries that
are only probably identical, on the one artefact whose behaviour defines
consensus. `maya_node_prebuilt_url` skips the build and installs a
checksum-verified artefact instead. For a real launch, use it.

### 4. Dry run — `scripts/local_cluster.sh`

```bash
./scripts/local_cluster.sh 12
```

Runs a real two-participant ceremony, verifies the state root, and starts twelve
real `node` processes from the resulting genesis on one host. It exercises the
software end to end; it does not exercise terraform, ansible, systemd, UFW or
TLS, and it does not pretend to.

Each node gets a 32 MiB block cache rather than the 512 MiB default — twelve at
the default would ask for 6 GiB of cache on one machine. If a node exits, the
script says how many survived rather than reporting on a cluster nobody asked
for.

## What has actually been verified

| Artefact | State |
|---|---|
| `terraform/` | `validate` and `fmt -check` pass; the mainnet guard was tested in both directions |
| `genesis-ceremony contribute` / `assemble` | run for real, two participants |
| `genesis --verify` | run for real against a ceremony's own output |
| `scripts/local_cluster.sh` | run locally |
| `infra/ansible/**` | **YAML parses. Nothing more.** |

Ansible is not installed on the machine this was written on, so the playbooks
have never been syntax-checked by `ansible-playbook --syntax-check`, let alone
run. They are a port of `deploy/deploy_bootstrap.sh`, which was itself carefully
written and also never executed. Treat both as drafts and dry-run them against a
disposable host. `docs/launch-checklist.md` tracks them with the other
generated-but-unverified artefacts.

No cloud resources were provisioned. There are no credentials on that machine
and `terraform plan` against a real provider was never run.
