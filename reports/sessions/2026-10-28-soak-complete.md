# 2026-10-28 — gate 3: seven days without a halt

Read from the public status service, `curl -s https://status.maya2c.dev/api/status`, 2026-10-27T15:58:34Z:

```json
{"genesis":1792511653,"halted_now":false,"halts":[],"height":366207,"last_block":1793116711,"since_last_block":2,"streak_seconds":605060,"threshold_seconds":60,"uptime":1.0}
```

- `streak_seconds` >= 604,800 (7 days); `halts` empty; `uptime` 1.0.
- Halt threshold: 60 s without a block (`threshold_seconds`).
- Topology: 1 Oracle validator (71% stake) + 4 PC validators, genesis `4ace93f6…` (2026-10-21).
- Caveat, stated publicly: the Oracle validator alone holds >2/3 of stake, so this soak shows the
  software runs a week without halting; it does not show the chain survives losing that host.
- Next: the re-genesis (docs/runbooks/regenesis-checklist.md) restarts the clock on the build
  carrying `bft.mining` (ADR-044).
