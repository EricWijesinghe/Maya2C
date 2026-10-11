# Maya2C node-v0.1.0-testnet.3: rejoin after anything, and a Windows validator

**Upgrade if you run a node on maya-testnet-1.** This release makes a
validator come back on its own after any length of outage, hardens peer
start-up, and adds a Windows installer so a validator can run on a Windows
machine, not only Linux. It is a testnet release: nothing on the chain
carries value.

> **Testnet, unaudited.** No external security audit has reported yet, and
> mainnet stays blocked until one has. The resilience results below come from
> our own attacknet and a local staging network, run by one operator on one
> machine. They are a floor, not an independent test, and a container or a
> second validator on the same PC is not a separate operator.

## Join

- **Linux:** `infra/testnet-vm/install.sh` in join mode (`GENESIS_URL`,
  `EXPECTED_SHA256`, `BOOTNODE`), or the one-line
  `scripts/join-testnet.sh`. Installs a sandboxed systemd service.
- **Windows (new):** `infra/testnet-vm/join-validator.ps1`, run once as
  Administrator. It verifies this release against `SHA256SUMS`, verifies the
  genesis against a published hash, keeps the validator key on the machine,
  and registers a clearly named boot task. It is transparent by design:
  visible in Task Scheduler, logs to `C:\ProgramData\Maya2C\logs`, and ships
  its own `-Uninstall`.

Many machines run by one person are still one operator — good for liveness
and stress testing, not a decentralisation claim.

## What's new since testnet.2

### Rejoin and consensus (ADR-043)
- **Rejoin after any outage.** A validator down past checkpoint retention
  catches up from a peer's snapshot and re-enters the committee on its own
  (attacknet attack 7, `reports/attacknet/2026-10-10-6.md`).
- **Validators serve snapshots by default**, so a far-behind or fresh node
  can bootstrap without replaying every block.
- **`--catch-up-from`** picks the live peer with the highest tip, repeatably.
- **Simultaneous start fixed.** Validators started together now dial from a
  fresh port, so a port-reuse/TIME_WAIT race no longer leaves them
  disconnected.

### Evidence toward the mainnet gates
- Gate 8 (validator rejoins), gate 9 (consensus logs pruned each epoch —
  measured 286 MB per validator, was 3.1 GB) and gate 10 (no cheap committee
  capture, a tested halt-recovery runbook) now carry evidence.

### Security and site
- **Per-IP RPC rate limiter** verified under a ~4,000 req/s flood from Kali:
  50 req/s served, the rest 429, consensus unaffected. DoS behaviour as
  designed (`reports/sessions/2026-10-11-stagenet-kali.md`).
- **Content-Security-Policy** on every page of maya2c.dev; unique titles,
  descriptions and structured data across the site; an interactive mainnet
  gates dial on the home page.
- Vulnerable dev dependencies patched (vitest, rustls, postcss override).

### Testing infrastructure
- **Container staging net** (`infra/stagenet/`): N validators, one container
  each, with `tc netem` WAN links on one PC. Explicitly not gate 4.
- **Attacknet lab** with real network-partition injection.

The binaries are not code-signed, so Windows and macOS warn about an unknown
publisher. Verify every download against `SHA256SUMS`.
