// Synthetic data for the Hub opened in a plain browser (no Tauri). The header
// says "Preview data" whenever this runs; the app build never calls it: its
// figures come only from the Rust commands.
"use strict";

// Synthetic data for a browser preview. `#paused` previews the paused network.
const preview = (() => {
  const rates = [];
  const events = [];
  const paused = location.hash === "#paused";
  let accepted = 0;
  let height = 241_000;
  let started = null;
  return {
    defaults: () => ({ rpc: "https://rpc.maya2c.dev/rpc", miner_binary: "maya2c-gpu-miner.exe", key_file: "C:\\Users\\you\\AppData\\Roaming\\dev.maya2c.hub\\miner.key",
      cpu_threads: 16, use_gpu: true, can_mine: true, thermal_ceiling_c: DEFAULT_CEILING_C,
      pools: [{ id: "public", name: "Maya2C public node", url: "https://rpc.maya2c.dev/rpc" }] }),
    miner_start: () => {
      started = started || Date.now();
    },
    miner_stop: () => {
      started = null;
    },
    miner_state: () => {
      const t = Date.now();
      if (started) {
        rates.push({ t_ms: t, hps: 6200 + 900 * Math.sin(t / 9000) + Math.random() * 400 });
        if (rates.length > 450) rates.shift();
        if (Math.random() < 0.05) {
          accepted += 1;
          events.push({ event: "share", accepted: true, reward: 7e22, t_ms: t });
        }
      }
      return {
        workers: [{ device: "gpu0", running: !!started, status: started ? "mining" : "stopped", mode: "preview", epoch: 118, share_reward: 7e22, accepted, rejected: 0,
          restarts: 0, held: false, temp_c: 60, ceiling_c: DEFAULT_CEILING_C, earned: accepted * 7e22, started_ms: started, address: "preview", rates, events }],
        alerts: [],
      };
    },
    node_snapshot: () => {
      if (paused) throw new Error("rpc: error sending request for url (https://rpc.maya2c.dev/rpc): connection refused (preview)");
      height += Math.random() < 0.6 ? 1 : 0;
      return { reachable: true, chain: { chain_id: "maya2c-preview", height }, fees: { active: true, base_fee: 1 },
        bft: { committee: 4, epoch: 118 }, mining: { active: true, epoch: 118, shares: 412, target_shares: 1000, share_reward: 7_000_000_000 },
        account: { balance: accepted * 7_000_000_000 } };
    },
    watch_balances: ({ addresses }) => addresses.map((input, i) => ({ input, address_text: input,
      balance: (BigInt(i + 1) * 1234567n * 10n ** 21n).toString(), nonce: i, error: null })),
    gpu_telemetry: () => ({ devices: [{ index: 0, name: "Preview GPU", temp_c: 58 + Math.random() * 4, power_w: 170 + Math.random() * 15,
      power_limit_w: 250, util_pct: 97, fan_pct: 54, mem_used_mib: 4300, mem_total_mib: 12227, clock_mhz: 2610 }], note: null }),
    notify_set: () => undefined,
  };
})();
