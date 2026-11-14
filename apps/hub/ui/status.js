// One reading of the network for every page: the node snapshot the Hub polls.
// Nothing in the Hub is called live unless the node answered the last poll;
// "built" is a fact about the code, "live" a fact about the network right now.
// Pure functions only, so apps/hub/ui-tests/logic.test.mjs can run them in node.
"use strict";

const NetStatus = (() => {
  // The public testnet was stopped on purpose; the date and the report are the
  // site's (docs/site/src/data/network-state.ts) and the session report's.
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

  // The chip one universe entry shows. Unknown stages read as planned, never live.
  // The card chip is short enough for a two-column card at 360 px; the legend
  // and the detail panel carry the date.
  function chip(item, net, chainId) {
    if (item.status !== "built") return { tone: STAGE[item.status] ? item.status : "planned", text: STAGE[item.status] || STAGE.planned };
    if (net === "live") return { tone: "live", text: liveText(chainId) };
    if (net === "connecting") return { tone: "built", text: "Built, checking" };
    return { tone: "paused", text: "Built · testnet paused" };
  }

  function legend(net, chainId) {
    const first = net === "live" ? { tone: "live", text: liveText(chainId) }
      : net === "connecting" ? { tone: "built", text: "Built" }
      : { tone: "paused", text: `Built, testnet paused since ${PAUSED_SINCE}` };
    return [first, ...Object.entries(STAGE).map(([tone, text]) => ({ tone, text }))];
  }

  // The calm card shown instead of "Node unreachable". The raw error is kept
  // for the Details disclosure and never written into the sentence.
  function pausedCopy({ rpc, error }) {
    const details = error ? String(error) : "";
    if (isPublic(rpc)) {
      return {
        title: "Testnet paused",
        body: `The public testnet is paused on purpose since ${PAUSED_SINCE}. No restart date is set; the Hub reconnects by itself when the node answers.`,
        report: true,
        details,
      };
    }
    return {
      title: "Your node is not answering",
      body: `No reply from ${hostOf(rpc)}. Check that it is running, or switch back to the public node in Settings.`,
      report: false,
      details,
    };
  }

  // Why Start cannot run right now, or null when it can.
  function startBlock({ net, canMine, devices, rpc }) {
    if (canMine === false) return "Mining runs on the desktop Hub; this one watches.";
    if (net === "connecting") return "Start waits for the first reply from the node.";
    if (net !== "live") {
      return isPublic(rpc) ? `Start waits for the network: the testnet is paused since ${PAUSED_SINCE}.`
        : `Start waits for your node at ${hostOf(rpc)} to answer.`;
    }
    if (!devices || !devices.length) return "Pick at least one device to mine on.";
    return null;
  }

  // The first-run sheet's last step: a backup promise and at least one device.
  const sheetReady = ({ backup, devices }) => backup === true && Array.isArray(devices) && devices.length > 0;

  return { PAUSED_SINCE, PUBLIC_RPC, STOP_REPORT, of, chip, legend, pausedCopy, startBlock, sheetReady, isPublic };
})();
