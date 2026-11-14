// One reading of the network for every page: the node snapshot the Hub polls.
// Nothing in the Hub is called live unless the node answered the last poll;
// "built" is a fact about the code, "live" a fact about the network right now.
// Pure functions only, so apps/hub/ui-tests/logic.test.mjs can run them in node.
"use strict";

const NetStatus = (() => {
  // The public testnet was stopped on purpose; the date and the link are those
  // of reports/sessions/2026-10-28-testnet-stopped.md.
  const PAUSED_SINCE = "2026-10-28";
  const PUBLIC_RPC = "https://rpc.maya2c.dev/rpc";
  const STOP_REPORT = "https://github.com/EricWijesinghe/Maya2C/blob/master/reports/sessions/2026-10-28-testnet-stopped.md";

  const STAGE = { building: "Building", research: "Research", planned: "Planned" };

  // "connecting" before the first reply, then "live" or "paused". Only a
  // literal `true` counts: a malformed snapshot is not a running network.
  function of(snapshot) {
    if (!snapshot) return "connecting";
    return snapshot.reachable === true ? "live" : "paused";
  }

  const isPublic = (rpc) => String(rpc || "").trim().replace(/\/+$/, "") === PUBLIC_RPC;

  const hostOf = (rpc) => {
    try {
      return new URL(rpc).host || String(rpc);
    } catch {
      return String(rpc || "the node in Settings");
    }
  };

  // One string for "live", so the legend and the cards never disagree.
  const liveText = (chainId) => (chainId ? `Live on ${chainId}` : "Live on testnet");

  // The tone and state one universe entry carries. Unknown stages read as
  // planned, never live. A built entry keeps the teal "built" tone while the
  // network is paused: the code exists, and the legend states the pause once.
  function chip(item, net, chainId) {
    if (item.status !== "built") return { tone: STAGE[item.status] ? item.status : "planned", text: STAGE[item.status] || STAGE.planned };
    if (net === "live") return { tone: "live", text: liveText(chainId) };
    if (net === "connecting") return { tone: "built", text: "Built, checking" };
    return { tone: "built", text: "Built · testnet paused" };
  }

  function legend(net, chainId) {
    const first = net === "live" ? { tone: "live", text: liveText(chainId) }
      : net === "connecting" ? { tone: "built", text: "Built" }
      : { tone: "paused", text: `Built, testnet paused since ${PAUSED_SINCE}` };
    return [first, ...Object.entries(STAGE).map(([tone, text]) => ({ tone, text }))];
  }

  // The one-line banner shown instead of "Node unreachable": the only place a
  // page says the network is paused. The raw error is kept for the Details
  // disclosure and never written into the sentence.
  function pausedCopy({ rpc, error }) {
    const details = error ? String(error) : "";
    if (isPublic(rpc)) {
      return {
        title: `Testnet paused since ${PAUSED_SINCE}`,
        body: "Stopped on purpose, no restart date set. The Hub reconnects by itself.",
        report: true,
        details,
      };
    }
    return {
      title: "Your node is not answering",
      body: `No reply from ${hostOf(rpc)}. Check it runs, or pick the public node in Settings.`,
      report: false,
      details,
    };
  }

  // Why Start cannot run right now, or null when it can. Short: the banner
  // above already says why the network does not answer.
  function startBlock({ net, canMine, devices, rpc }) {
    if (canMine === false) return "Mining runs on the desktop Hub; this one watches.";
    if (net === "connecting") return "Waits for the first reply from the node.";
    if (net !== "live") return isPublic(rpc) ? "Waits for the network." : `Waits for your node at ${hostOf(rpc)}.`;
    if (!devices || !devices.length) return "Pick at least one device to mine on.";
    return null;
  }

  // The first-run sheet's last step: a backup promise and at least one device.
  const sheetReady = ({ backup, devices }) => backup === true && Array.isArray(devices) && devices.length > 0;

  return { PAUSED_SINCE, PUBLIC_RPC, STOP_REPORT, of, chip, legend, pausedCopy, startBlock, sheetReady, isPublic };
})();
