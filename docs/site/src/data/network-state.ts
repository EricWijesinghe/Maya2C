// Whether the public testnet is stopped on purpose, and the last reading
// before it stopped. One flag for the whole site: the maintenance overlay,
// the hero chip, the header's Explorer and Status links (W-03) and every live
// tile read it. Set `paused` to false with the next genesis.
//
// Last reading: reports/sessions/2026-10-28-testnet-stopped.md, status.maya2c.dev
// /api/status at 2026-10-28 ~07:36 UTC. Copied from that report, never typed fresh.
export const testnet = {
  paused: true,
  since: "2026-10-28",
  report: "https://github.com/EricWijesinghe/Maya2C/blob/master/reports/sessions/2026-10-28-testnet-stopped.md",
  last: {
    height: 393812,
    at: "2026-10-28 07:36 UTC",
    // Genesis 1792511653 and last block 1793172992 in the same report.
    from: "2026-10-20",
    to: "2026-10-28",
    run: "7 d 15 h 42 min",
    halts: 0,
    uptime: 1.0,
    // `threshold_seconds` in the same reading: a halt is any gap over this.
    haltSeconds: 60,
  },
} as const;

/** The text a live tile shows while paused: the last value with its date, or "paused since". */
export const pausedLabel = `paused since ${testnet.since}`;
