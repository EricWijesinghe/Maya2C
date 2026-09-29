# Maya2C node-v0.1.0-testnet.1: the first public testnet

**maya-testnet-1 is live.** This is a post-quantum layer-1 chain with DAG-BFT
finality, in Rust from the key to the chain. With this release you can run
it, use it and build on it today, with free test coins. Nothing on it
carries value.

> **Testnet, unaudited.** No external security audit has reported yet, and
> mainnet stays blocked until one has (`cargo xtask go-no-go`). Don't use
> anything here to protect real money or secrets.

## Get started in two minutes

1. **Download Maya Wallet** for your system from the assets below, and check
   it against `SHA256SUMS`.
2. **Get free test coins** from the faucet: https://faucet.maya2c.dev, or the
   form on https://maya2c.dev/guides/testnet/.
3. **Send a post-quantum-signed transaction.** Every transfer carries an
   ML-DSA-65 and an SLH-DSA signature, and both must verify.
4. **Open Maya Chat** for end-to-end post-quantum messaging, with the key in
   your operating system's keychain.

The full guide is at https://maya2c.dev/guides/testnet/.

## What's in this release

### Chain
- **DAG-BFT finality** (Narwhal + Bullshark): a block is derived from committed
  certificates, never proposed, and a validator restarted from its safety log
  cannot equivocate.
- **Staking and slashing.** Registration joins the committee at the next
  epoch, and equivocation evidence removes and slashes the validator. The
  stolen-key incident is rehearsed in the test suite.
- **Fee market.** The base fee is burned and validators earn tips.
  **Emission is nil**: there is no block reward, and no one earns by mining.
- **Remote validator signer**, opt in with `--remote-signer`. The validator
  key lives in a separate `maya2c-signer` process with slashing protection per
  `(round, author)` slot (ADR-033). It hasn't been externally reviewed.
- **Production node build**: `--features production` runs DAG-BFT only.
- **Contract VM hardening.** Reference types and typed function references are
  disabled, so contracts cannot reach the wasmtime fuel-accounting bug
  RUSTSEC-2026-0315. They had been silently enabled; tests now pin each
  disabled proposal (ADR-034).

### Cryptography
- **NIST post-quantum standards**: ML-KEM (FIPS 203), ML-DSA (FIPS 204) and
  SLH-DSA (FIPS 205). They pass **324 official NIST ACVP test cases** plus the
  X-Wing and HQC reference vectors.
- **Post-quantum transport**: libp2p Noise with an ML-KEM-768 layer.

### Apps and tools
- **Maya Wallet**, a desktop app for Windows, macOS and Linux, and
  `l1-wallet` on the command line. Fees are priced from the chain.
- **Maya Chat**, a desktop app for Windows, macOS and Linux:
  - ML-DSA identities and X-Wing (ML-KEM-768 + X25519) handshakes;
  - a fresh key per message;
  - relays that see only ciphertext, and proof-of-work postage against spam.
- **SDKs** for TypeScript, Python and Go, tested against a live chain.
- **API gateway**: REST and GraphQL on `/v1`, plus JSON-RPC on `/rpc`
  restricted to a list of allowed methods. It fronts the node, which stays
  private.
- **Faucet**: rate limits and a daily budget that survive a restart.

### Run it yourself
- `cargo xtask up` runs the whole ecosystem on one machine in one command:
  four validators, the gateway, a chat relay and a faucet.
- `infra/testnet-vm/install.sh` turns a fresh Ubuntu VM into a public node
  with one command. It sandboxes every service, keeps the node RPC private,
  and serves HTTPS through Caddy. `infra/oracle-free/` sets up the VM on
  Oracle's free tier.

## Network

| | |
|---|---|
| Chain | `maya-testnet-1` |
| Genesis state root | `bf084e888a1c4fc4e396e846ff4cdeab5919155b41e463177886d3c36aa21143` |
| Genesis block | `07f141151158440c1e491eb93e503e6981f2f439be50353ea05384f67a54ccf7` |
| API | https://rpc.maya2c.dev (`/v1` REST, `/rpc` JSON-RPC) |
| Faucet | https://faucet.maya2c.dev |
| Guide | https://maya2c.dev/guides/testnet/ |

## Downloads

| | Windows | macOS | Linux |
|---|---|---|---|
| Maya Wallet | `.msi`, `-setup.exe` | `.dmg` | `.deb`, `.AppImage` |
| Maya Chat | `.msi`, `-setup.exe` | `.dmg` | `.deb`, `.AppImage` |
| Node and CLI | `maya2c-node`, `l1-wallet`, `maya2c-peerid` for Linux (x86_64, aarch64), Windows and macOS | | |

**Verify every download:** `sha256sum -c SHA256SUMS`. The installers are **not
code-signed yet**, so Windows SmartScreen and macOS Gatekeeper warn about an
unknown publisher.

## Honest limits

- **One validator.** Finality works, but the network is not decentralised:
  if the seed stops, the chain stops. Tolerating a fault takes four
  independent validators (LAUNCH.md).
- **No public P2P yet.** While the seed runs behind a tunnel, reach the
  chain through the API. Outside operators join once it moves to a VM.
- **Unaudited.** That covers the cryptography integration, Maya Chat's
  protocol, the remote signer and the gateway. The audit is the next gate.
- **Chat metadata.** Relays see who receives how much and when. Mixnet
  routing is planned.

Everything above is backed by a test in the repository, and
`features.toml` records which is which. Report issues on GitHub.
