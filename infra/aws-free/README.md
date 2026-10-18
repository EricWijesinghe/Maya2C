# infra/aws-free — maya-testnet-1 off the owner's PC

Four validators, one per AWS instance, so unplugging the PC no longer stops
the chain. Quorum is n − f = 3: any **one** instance can fail or reboot
without a halt; two at once halts it, like the PC setup before.

| Host | Type | Runs |
|---|---|---|
| v1 | t4g.small (2 GB) | validator, gateway (rpc), bootstrap, explorer, status, chat relay, tunnel connector |
| v2–v4 | t4g.micro (1 GB) | validator |

Spread over three availability zones of ap-southeast-2 (the project's only
Region). No SSH and no key pair: hosts are reached through SSM Session
Manager. RPC is never public; only libp2p TCP 31100 is open.

## Cost (prices read from the AWS Pricing API, 2026-10-18)

| Item | Monthly |
|---|---|
| 3 × t4g.micro at $0.0106/h | $23.21 |
| 1 × t4g.small at $0.0212/h | $15.48 |
| 4 × 20 GB gp3 at $0.096/GB-month | $7.68 |
| 4 public IPv4 at $0.005/h | $14.60 |
| **Total** | **≈ $61** |

The $100 of free-plan credits last about **7 weeks**. CPU credits are
"unlimited", which bills any burst above baseline, so the real figure is
higher by whatever the nodes burn; watch Billing. When the credits run out
(or the free plan reaches 2027-04-18) AWS stops the project's resources
unless it has been upgraded to the paid plan — and the chain halts with them.
Decide before then.

## Keys

Each host generates its own validator key, **sealed from the first byte**
(`--generate-validator-key` with `--validator-passphrase-file`), and a random
passphrase in `/etc/maya2c/validator.passphrase` (root only), handed to the
node as a systemd credential. Neither leaves the host. Losing a host loses
its key; the committee keeps quorum with three.

v1 takes the PC seed's `node_key` so the published bootnode peer id stays
valid. The tunnel credentials are uploaded by the owner to
`secret/tunnel.json`; v1 deletes the object as soon as it has read it.

## Steps

1. `aws cloudformation deploy --template-file stack.yaml --stack-name maya2c-testnet --capabilities CAPABILITY_IAM --parameter-overrides VpcId=… SubnetA=… SubnetB=… SubnetC=…`
2. Upload to the bucket: `release/maya2c-aarch64-unknown-linux-gnu.tar.gz`,
   `release/maya2c-services-aarch64-unknown-linux-gnu.tar.gz` (from a manual
   run of `release-binaries.yml`), `release/explorer-assets.tar.gz`,
   `release/node-config.toml`, the scripts, `secret/node_key`.
3. On each host: `setup-node.sh keys <N> <bucket>`.
4. Build genesis from the previous one with the four new keys from
   `public/v*.pub` (same allocations and bond operators, new timestamp);
   check it with `maya2c-genesis`; upload `release/genesis.json` and
   `release/bootnodes.txt` (`<N> /ip4/<private ip>/tcp/31100/p2p/<peer id>`).
5. Stop the PC chain and its tunnel connector.
6. On all four, close together: `setup-node.sh start <N> <bucket>`.
7. Owner uploads the tunnel credentials; on v1:
   `setup-services.sh <bucket> <cloudflared version>`.
