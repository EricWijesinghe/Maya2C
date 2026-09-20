# Maya2C multi-region deployment runbook

How to stand up the fleet across US, EU, and Asia on AWS, GCP, or both — and
what still stands between that fleet and a value-bearing mainnet.

---

## 1. Mainnet is blocked, and the block is deliberate

The shielded pool's Groth16 parameters come from a reproducible test setup, not
from a ceremony. Anyone able to re-run that setup holds the toxic waste, and
with it can mint shielded value that no supply audit would reveal — including
the supply endpoints this repo serves to CoinGecko and CoinMarketCap.

Three independent places refuse a value-bearing chain:

| Where | What happens |
|---|---|
| `crates/zk-privacy/src/prove.rs:48` | `SETUP_IS_TRUSTED = false` |
| `crates/node/src/bin/node.rs` (`VALUE_BEARING_CHAINS`) | The node exits at startup on `mainnet` / `maya-mainnet` |
| `infra/terraform/modules/*/variables.tf` | `terraform plan` fails for those chain ids, in both clouds |
| `crates/zk-privacy/tests/proof_tests.rs:55` | A test asserts the flag stays false |

**Everything in this runbook deploys a non-value-bearing chain id.** That is a
real, fully multi-region network — it is simply not one anybody should put money
on yet.

To lift the block you need a multi-party ceremony where at least one participant
is honest and destroys their contribution, the resulting parameters committed and
independently verified, and only then `SETUP_IS_TRUSTED` flipped and the test
updated. That is calendar time and external participants, not a code change.
Nothing else in this document depends on it.

---

## 1a. Transaction authorization is hybrid ML-DSA-65 + SLH-DSA-SHA2-128s, and
that is a hard fork

Separate from the ceremony, and easy to trip over because it changes values that
look unchanged.

Every transaction now carries **two** signatures over the same bytes: a lattice
proof under FIPS 204 (ML-DSA-65) and a hash-based proof under FIPS 205
(SLH-DSA-SHA2-128s). Both must verify, at mempool admission and again at block
execution. There is no wire version, configuration flag, or legacy path that
accepts one.

### Why two

The two schemes are post-quantum for unrelated reasons, and that is the entire
point. ML-DSA rests on Module-LWE, a structured lattice assumption about fifteen
years old — believed hard, not proven hard, and the ring structure that makes it
fast is what a future attack would attack. SLH-DSA rests on nothing but the
preimage and collision resistance of SHA-2. A forgery must break lattices *and*
hashes; a cryptanalytic advance against either alone leaves the ledger intact.

### Operational consequences

| What changed | Consequence |
|---|---|
| An address is now `blake3(v3 ‖ ml_dsa_pk ‖ slh_dsa_pk)` | Every address changes **again**. Genesis allocations are still 32 bytes of hex, so an old allocation file is **structurally valid and semantically wrong** — it names accounts nobody holds keys for |
| Wire versions 1–4 are refused by name | Any stored transaction or block from before this fork is unusable. This is a new chain, not an upgraded one. v3 and v4 carry a perfectly good ML-DSA signature and are refused anyway, because one valid proof is not the rule |
| The genesis chain-id domain moved to v2 | An old and a new node cannot agree on the genesis hash, so they cannot silently disagree about who owns the premine |
| Keystores are version 4 | `l1-wallet` refuses 1–3 with an explanation. Version 3 is the one that looks upgradable and is not: its ML-DSA key is valid, but pairing it with a fresh SLH-DSA key names a *different* address holding nothing |

Get an address for an allocation with `l1-wallet address`, which prints the
derived address rather than a key.

The mnemonic-to-account derivation remains **Maya2C-specific**, and has changed.
The BIP-39 phrase and SLIP-0010 path are standard, but no standard defines
hierarchical derivation for either ML-DSA or SLH-DSA, so one chain key is split
into two domain-separated scheme seeds. The same words now name different
addresses than they did before this fork, and recovering them in another wallet
finds nothing.

### Sizes and costs, before setting any limit

Measured by `cargo bench --bench hybrid_signing` and
`cargo bench --bench hybrid_footprint`:

| | ed25519 | ML-DSA-65 | hybrid |
|---|---|---|---|
| public key | 32 B | 1952 B | 1984 B |
| signature | 64 B | 3309 B | 11165 B |
| one-output transfer | ~200 B | ~5.3 KB | **13215 B** |
| transfers per 8 MiB gossip message | ~40000 | 1574 | **634** |
| channel closure | 216 B | 10610 B | **26386 B** |
| sign | 26 µs | 0.22 ms | **105 ms** |
| verify | 15 µs | 0.099 ms | **0.18 ms** |

Two things to take from that table.

**Verification barely moved.** 0.18 ms against 0.099 ms, and a *rejected*
signature costs 0.070 ms because the check short-circuits at the lattice half.
A 64-transaction block executes end to end in 26 ms. Block validation was never
the constraint and still is not.

**Signing and bytes moved a great deal.** 105 ms per signature is a wallet-side
cost paid once per transaction, which is acceptable; but it made the L2 scale
tests unrunnable at their previous volume, and `TOTAL_TRANSFERS` in
`crates/l2-flash/tests/scale_tests.rs` was cut from 1000 to 20 as a result. The
full-volume run is still reachable under `cargo test -- --ignored` and takes
roughly an hour.

`MAX_BATCH_CLOSURES` is held at 128 and is now the **binding** constraint rather
than a generous one: a full batch is ~3.2 MiB of the 8 MiB gossip ceiling, so
two such transactions fit in a block and a third does not. It was held at 128
rather than cut because the hundred-channel batch the L2 scale tests settle is a
property the system claims, and a limit that broke it would be choosing a round
number over a claim.
---

## 2. Prerequisites

The manifests assume these are already installed in each cluster. They are not
managed here, because each is a cluster-wide concern that usually outlives any
one application.

| Component | Needed by |
|---|---|
| ingress-nginx | `infra/k8s/base/ingress.yaml` — both Ingress objects and every rate-limit annotation |
| cert-manager, with a `letsencrypt-prod` ClusterIssuer | the `maya-rpc-tls` certificate |
| external-dns | publishes the per-seed p2p hostnames the bootnode multiaddrs are written against |
| Prometheus Operator CRDs | `infra/k8s/base/servicemonitor.yaml` — the ServiceMonitor and PrometheusRule |
| AWS Load Balancer Controller | AWS only; the per-seed `LoadBalancer` Services. GKE needs no equivalent |

A namespace labelled `monitoring` and one labelled `ingress-nginx` must exist —
the NetworkPolicy selects on `kubernetes.io/metadata.name`, and a policy whose
selector matches nothing silently denies rather than allows.

---

## 3. Terraform state

State is remote and locked. Three regions live in one state file, which makes
every apply a fleet-wide operation — and a fleet-wide operation against state
that exists only on one laptop is one lost disk away from a fleet nobody can
modify or destroy.

Create the bucket once, out of band:

```bash
aws s3api create-bucket --bucket maya2c-tfstate --region us-east-1
aws s3api put-bucket-versioning --bucket maya2c-tfstate \
    --versioning-configuration Status=Enabled
aws s3api put-public-access-block --bucket maya2c-tfstate \
    --public-access-block-configuration \
    BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
```

Versioning is not optional. It is the only thing that turns a corrupted or
truncated state write into an inconvenience.

```bash
terraform -chdir=terraform init \
    -backend-config=bucket=maya2c-tfstate \
    -backend-config=region=us-east-1
```

Locking uses S3's native lock file (`use_lockfile = true`), so no DynamoDB table
is required.

---

## 4. Provision

### AWS only

```bash
terraform -chdir=terraform apply -var chain_id=maya-testnet
```

### Both clouds

```bash
terraform -chdir=terraform apply \
    -var chain_id=maya-testnet \
    -var enable_gcp=true \
    -var gcp_project=your-project-id
```

`enable_gcp` defaults to false so an AWS-only operator never needs GCP
credentials and a plan without them does not fail.

### Cost worth knowing before you apply

| Item | Default | Note |
|---|---|---|
| NAT gateways | 1 per AWS region (3 total) | `single_nat_gateway=false` gives one per zone — 9 total, several hundred dollars a month of standing charge, bought against a single-zone NAT failure |
| Network load balancers | 3 per region | One per seed. Required: a peer must dial a *specific* identity, so one balancer across three pods cannot work |
| Inter-region transfer | per block, per peer | The standing price of a mesh over the public internet |

### Get credentials

```bash
aws eks update-kubeconfig --region us-east-1      --name maya-us
gcloud container clusters get-credentials maya-gcp-us --region us-central1
```

`terraform output clusters` lists every cluster and endpoint across both clouds.

---

## 5. Seed identities — do this before deploying

A seed's `PeerId` comes from `<data-dir>/node_key`, and the other regions'
bootnode multiaddrs name that `PeerId`. Left to itself each pod would mint a key
on first boot, which means the addresses could only be written after the fleet
was already running — and would go stale the moment a volume was replaced.

So generate them first.

```bash
# Nine identities: three seeds in each of three regions.
for region in us eu asia; do
  for i in 0 1 2; do
    mkdir -p seeds/$region-$i
    echo "$region-$i $(cargo run --quiet --bin peerid -- --data-dir seeds/$region-$i --create)"
  done
done
```

Each directory now holds a 32-byte `node_key`. `seeds/` is gitignored; treat it
as key material and store it wherever your secrets live.

Load one region's keys into its cluster, named by pod:

```bash
kubectl -n maya-us create secret generic maya-seed-identity \
    --from-file=maya-seed-0=seeds/us-0/node_key \
    --from-file=maya-seed-1=seeds/us-1/node_key \
    --from-file=maya-seed-2=seeds/us-2/node_key
```

The init container installs the key matching the pod's name, and **never
overwrites one already on the volume** — once a seed has run, the PVC holds the
identity the rest of the fleet dials.

The Secret is optional. Without it every node generates its own key, which is
the right behaviour for a throwaway network where nothing has pinned the
`PeerId`s yet.

---

## 6. Substitute the placeholders

The repo ships `.invalid` hostnames and throwaway `PeerId`s. Nothing can resolve
or connect as written — that is the point, but it means these edits are
mandatory, not cosmetic.

| File | Replace |
|---|---|
| `infra/k8s/overlays/*/kustomization.yaml` | `rpc-*.example.invalid`, `seed-*.p2p.example.invalid` |
| `infra/k8s/overlays/*/bootnodes.yaml` | the same p2p hostnames, and every `/p2p/12D3KooW...` with the real `PeerId` from step 5 |
| `infra/k8s/base/configmap.yaml` | `genesis.json` — chain id, timestamp, difficulty, allocations |

The p2p hostnames appear in two places per region: the `external-dns` annotation
that publishes them, and the other regions' bootnode multiaddrs that dial them.
They must agree exactly.

The `/p2p/` component is not decoration. Without it a dial authenticates nothing
— the node connects to whatever answers and accepts the identity it is handed.
With it, a hijacked DNS record is a failed dial rather than a hostile peer.

---

## 7. Deploy, one region at a time

```bash
kubectl apply -k infra/k8s/overlays/us
kubectl -n maya-us rollout status statefulset/maya-seed --timeout=10m
```

Then EU, then Asia. One at a time because each region's seeds bootstrap from the
other two: bringing all three up simultaneously means every bootnode dial fails
until the last region is ready, which converges eventually but makes a genuine
failure indistinguishable from ordinary startup noise.

GCP overlays are `infra/k8s/overlays/gcp-us`, `gcp-eu`, `gcp-asia`.

### Verify

```bash
# Peers. Anything below 2 means this node is effectively isolated.
kubectl -n maya-us exec maya-seed-0 -- \
    wget -qO- localhost:9600/metrics | grep maya_peers_connected

# One network, not three: every region must report the same genesis.
curl -s https://rpc-us.example.invalid   -d '{"jsonrpc":"2.0","id":1,"method":"get_block_by_height","params":[0]}'
curl -s https://rpc-eu.example.invalid   -d '{"jsonrpc":"2.0","id":1,"method":"get_block_by_height","params":[0]}'
curl -s https://rpc-asia.example.invalid -d '{"jsonrpc":"2.0","id":1,"method":"get_block_by_height","params":[0]}'

# Supply endpoints, in the shape the aggregators fetch.
curl -s https://rpc-us.example.invalid/api/v1/total_supply
curl -s https://rpc-us.example.invalid/api/v1/circulating_supply
```

The two supply endpoints must return a bare decimal and nothing else. A JSON
wrapper, a unit suffix, or a trailing newline each break the aggregators' parse.

---

## 8. Aggregator submission

CoinGecko and CoinMarketCap take two things from a project's own infrastructure:

| Endpoint | Returns |
|---|---|
| `/api/v1/total_supply` | bare decimal, base units |
| `/api/v1/circulating_supply` | bare decimal, base units |

Both are in **base units** — the same unit as account balances and transfer
amounts. There is no decimals constant in this codebase and nothing is scaled.
If one is ever introduced, tell both aggregators the exponent in the same change:
an unannounced change of unit shows up as a supply figure wrong by orders of
magnitude, and that gets a listing flagged rather than corrected.

The ticker endpoints — `/api/v1/ticker` (CoinGecko shape) and
`/api/v1/cmc/ticker` (CoinMarketCap shape) — are adapters over an
operator-supplied feed, not a data source. A node has no price; there is none
until the asset trades somewhere. With no feed configured they answer `503`,
which says "ask again" rather than the "trades nowhere" an empty array would
imply.

To populate them once the asset is listed on an exchange, supply a JSON array of
quotes and point the node at it:

```json
[
  {
    "pair": "MAYA_USDT",
    "last_price": 1.25,
    "base_volume": 1000.0,
    "quote_volume": 1250.0
  }
]
```

```
--market-feed /config/market-feed.json
```

A file that exists but cannot be parsed is fatal at startup, on purpose: falling
back to an empty feed would leave the endpoints answering `503` while the
operator believed they were configured.

---

## 9. Rotating a seed identity

Rotation invalidates every bootnode address pointing at that seed, so it is a
fleet-wide change, not a per-pod one.

1. Generate the new key: `peerid --data-dir seeds/us-0-new --create`.
2. Update the Secret in that region.
3. Update the `/p2p/` component in **both** other regions' `bootnodes.yaml`.
4. Delete the seed's PVC — the init container will not overwrite an existing
   key, which is exactly the protection that makes this step explicit.
5. Roll the pod, then roll the other two regions to pick up the new address.

Rotate one seed at a time. The PodDisruptionBudget holds a floor of two
available seeds; taking two out at once drops the region below the point where a
restarting node can find a peer.

---

## 10. Alerts worth knowing

From `infra/k8s/base/servicemonitor.yaml`:

| Alert | Why it matters |
|---|---|
| `MayaNodeIsolated` | Fewer than two peers. The process is up and RPC answers — with an increasingly stale tip. This is the failure that looks like health |
| `MayaChainStalled` | Height flat for 30 minutes. Watches height rather than the import counter, because a healthy solo miner imports nothing |
| `MayaSlowBlockImport` | p95 import above one second. Measured against one clock, so it cannot fire because someone's NTP drifted |
| `MayaShieldedSupplyAnomaly` | A large negative delta in pool balance. The pool balance is derived from public flows; a drop suggests a deep reorg or a proof-system failure |

Alert on `maya_block_import_duration_seconds`, not
`maya_block_observed_age_seconds`. The second includes clock skew between miner
and observer, so it pages on somebody else's broken clock.

---

## 11. Checklist

- [ ] Ceremony run, `SETUP_IS_TRUSTED` flipped — **only if deploying a value-bearing chain**
- [ ] State bucket created, versioned, public access blocked
- [ ] Cluster prerequisites installed in every region (section 2)
- [ ] Nine seed identities generated and stored as Secrets
- [ ] Every `.invalid` hostname replaced
- [ ] Every placeholder `PeerId` replaced
- [ ] `genesis.json` identical in all six overlays — a mismatch forks the chain silently
- [ ] All regions report the same genesis block
- [ ] Every node reports two or more peers
- [ ] Supply endpoints return bare decimals
- [ ] Prometheus is scraping all regions
