// What every guide says while the public testnet is stopped, in one place
// (lane L04). Everything here derives from network-state.ts, so flipping
// `testnet.paused` with the next genesis turns every paused label, disabled
// button and recorded response back into the live version at once.
import { testnet } from "./network-state.ts";

/** The hosts that answer only while the testnet runs. */
export const PUBLIC_HOSTS = ["faucet.maya2c.dev", "rpc.maya2c.dev", "explorer.maya2c.dev", "status.maya2c.dev"] as const;

/** The path that works today: a one-validator devnet on your own machine. */
export const LOCAL_DEVNET_HREF = "/guides/quickstart/";

/** Tab label for commands that reach a public host. */
export const publicTab = testnet.paused ? "Public testnet (paused)" : "Public testnet";

/** Why a control that reaches a public host is switched off, or null when live. */
export const pausedReason: string | null = testnet.paused
  ? `Paused since ${testnet.since}: the public testnet is stopped until the next genesis, so there is nothing for this to reach. Use a local devnet instead.`
  : null;

/**
 * The last reading before the stop, verbatim from the stop report (a test
 * checks the report still contains this exact line). Shown by the /experience/
 * playground in place of a live call.
 */
export const recorded = {
  source: "status.maya2c.dev/api/status",
  at: "2026-10-28 ~07:36 UTC",
  body: '{"genesis":1792511653,"halted_now":false,"halts":[],"height":393812,"last_block":1793172992,"since_last_block":18,"streak_seconds":661357,"threshold_seconds":60,"uptime":1.0}',
} as const;

/** The two caveats recorded with the 7-day result, and what a halt is. */
export const runCaveats = {
  stake: "one validator held 71% of stake, so it finalized alone",
  build: "the run was on the pre-mining build",
  halt: "a halt is any gap over 60 s between blocks",
} as const;
