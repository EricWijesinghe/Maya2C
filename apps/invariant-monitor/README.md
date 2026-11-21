# maya2c-invariant-monitor

Watches one or more public Maya2C endpoints and reports the invariant
violations a third party can see from public RPC. Read-only: it calls getters, never signs or
submits.

```sh
maya2c-invariant-monitor --node gw=https://<gateway>/rpc --node own=http://127.0.0.1:8545
maya2c-invariant-monitor --node gw=https://<gateway>/rpc --once   # exit 1 on a violation
```

Findings print to stdout. Set `MAYA2C_MONITOR_NTFY_TOPIC` to also push them to
ntfy (rate-limited as the sentinel's are); `--dry-run` never pushes.

| Check | From | Severity |
|---|---|---|
| halt, fork at one height, supply adds up, supply split, supply fell | `apps/sentinel` detectors | as the sentinel |
| height never goes backwards per source | `get_chain_info` | warning |
| newest checkpoint never goes backwards per source | `get_checkpoint` | warning |
| finality never reverts: one block id per attested height, across sources and against the served block | `get_checkpoint`, `get_block_by_height` | critical |
| finalized block within `MAX_BLOCK_BYTES` (invariant 33) | `get_block_by_height` `raw` | critical |
| the block at the checkpoint height is served | `get_block_by_height` | warning |

Not checkable through the gateway, so not checked: checkpoint signatures and
quorum weight (committee keys are not public), the invariant guard (28),
peer floor and stranding (`get_peers` and `get_mempool_info` are not on the
allowlist). The revert check remembers the newest 4,096 attested heights it has
seen, in memory, and evicts the lowest.

Limits, stated so nobody reads more into a quiet run:

- The revert and byte-cap checks look at the newest checkpoint each poll, not
  every block finalized between polls.
- The first attestation heard for a height is the reference; signatures are
  not verified. A lying source heard first makes honest ones look like a
  revert. Either way the alert means the sources disagree about finality.
- `--once` has no history: the height and checkpoint regress checks and the
  halt timer cannot fire. Fork, supply, revert across sources and the byte cap
  can.
