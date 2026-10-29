# 2026-10-28 — maya-testnet-1 stopped on purpose after the 7-day soak

Eric: "if 7day testnet over stop it and upgrade the progress with proof in everywhere".
Gate 3 had passed at 604,832 s (`2026-10-28-soak-complete.md`). The chain was then
stopped deliberately so the next build can start from a fresh genesis.

## Last reading before the stop (status.maya2c.dev/api/status, 2026-10-28 ~07:36 UTC)

```json
{"genesis":1792511653,"halted_now":false,"halts":[],"height":393812,"last_block":1793172992,"since_last_block":18,"streak_seconds":661357,"threshold_seconds":60,"uptime":1.0}
```

- Genesis 1792511653 = 2026-10-20 15:54:13 UTC. Last block 1793172992 = 2026-10-28 07:36:32 UTC.
- 661,357 s = 7 d 15 h 42 min, blocks 0 → 393,812, `halts []` (a halt is any gap over 60 s),
  uptime 1.0.
- Checkpoint read from the Oracle validator a minute earlier (`get_checkpoint` on 18601):
  epoch 109, height 393,808, block `39708092c4067270d3b4877d7f99906adff5c64f9f7746f6458cb328daae868a`.

## How it was stopped (by PID and unit, never by image name)

- PC: scheduled task `Maya2C testnet watchdog` disabled (it restarts validators), then
  `Stop-Process -Id` 60544, 25404, 67480, 7240 (pc4, pc1, pc2, pc3): `stopped` ×4.
- Oracle VM: `sudo systemctl disable --now maya2c-watch.timer maya2c-validator@1` →
  both `inactive`. Journal: `07:36:38 Stopped maya2c-validator@1.service`.
- Left running: the observer peer `maya2c-peer@11` on the VM (non-voting), and the PC
  services (gateway, explorer, faucet, status, chat relay) that serve the frozen history.
  The status page now shows a halt from 07:36:32 UTC: that is this deliberate stop.
- Data kept: `D:\Maya2C-validators\pc\pc{1..4}\data` and the VM's data dir. Not deleted.

## Caveat (stated with the result)

One Oracle validator held 71% of stake and finalized alone; the four PC validators were never
needed for quorum. ADR-044 requires the soak again on the mining build; it will overlap the
self-run attacknet and pre-audit.

## Next

Re-genesis needs `APPROVED: re-genesis` (`docs/runbooks/regenesis-checklist.md`) once #218
(u256), #221 (forever mining) and the ML-DSA-87 validator keys are in.
