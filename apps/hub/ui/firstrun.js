// The first-start sheet: what mining does to this computer, where the payout
// key lives, which devices run. No miner starts from the Hub until this sheet
// has been finished once with the backup box ticked (NetStatus.sheetReady).
"use strict";

const FirstRun = (() => {
  const KEY = "hub.firstRunDone";
  const STEPS = 3;
  const $ = (id) => document.getElementById(id);
  let host = null; // { call, isPreview, settings, gpus, label, net, onFinish }
  let step = 0;

  function done() {
    try {
      return localStorage.getItem(KEY) === "1";
    } catch {
      return false; // blocked storage: the sheet shows on every launch
    }
  }

  function markDone() {
    try {
      localStorage.setItem(KEY, "1");
    } catch {
      // The sheet will show again next launch; nothing else depends on it.
    }
  }

  const picked = () => [...$("fr-devices").querySelectorAll("input:checked")].map((box) => box.value);

  function renderDevices() {
    const gpus = host.gpus();
    const keys = ["cpu", ...gpus.map((g) => `gpu${g.index}`)];
    const chosen = host.settings().devices || keys.filter((k) => k !== "cpu" || !gpus.length);
    $("fr-devices").replaceChildren(
      ...keys.map((key) => {
        const label = document.createElement("label");
        label.className = "pick-item";
        const box = document.createElement("input");
        box.type = "checkbox";
        box.value = key;
        box.checked = chosen.includes(key);
        const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        icon.setAttribute("class", "icon");
        icon.setAttribute("aria-hidden", "true");
        const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
        use.setAttribute("href", "icons.svg#chip");
        icon.append(use);
        const text = document.createElement("span");
        text.textContent = host.label(key);
        label.append(box, icon, text);
        return label;
      }),
    );
  }

  // The last step's button: enabled only with the backup box and a device.
  function refresh() {
    const devices = picked();
    const ready = NetStatus.sheetReady({ backup: $("fr-backup").checked, devices });
    const block = NetStatus.startBlock({ net: host.net(), canMine: true, devices, rpc: host.settings().rpc });
    $("fr-start").disabled = !ready;
    // While the network is paused the sheet still saves; Start waits on the hero.
    setText($("fr-start"), block && devices.length ? "Finish" : "Start mining");
    const why = !$("fr-backup").checked ? "Tick the backup box on the previous step first."
      : !devices.length ? "Pick at least one device."
      : block || "";
    setText($("fr-why"), why);
  }

  function setText(el, text) {
    if (el.textContent !== text) el.textContent = text;
  }

  function show(next) {
    step = Math.max(0, Math.min(STEPS - 1, next));
    document.querySelectorAll("#first-run [data-step]").forEach((el) => {
      el.hidden = Number(el.dataset.step) !== step;
    });
    document.querySelectorAll("#first-run [data-step-dot]").forEach((el) => {
      const n = Number(el.dataset.stepDot);
      el.classList.toggle("on", n <= step);
      if (n === step) el.setAttribute("aria-current", "step");
      else el.removeAttribute("aria-current");
    });
    $("fr-back").hidden = step === 0;
    $("fr-next").hidden = step === STEPS - 1;
    $("fr-start").hidden = step !== STEPS - 1;
    refresh();
    const heading = document.querySelector(`#first-run [data-step="${step}"] .sheet-title`);
    if (heading) {
      heading.tabIndex = -1;
      heading.focus();
    }
  }

  function open() {
    const s = host.settings();
    setText($("fr-ceiling"), String(s.thermal_ceiling_c ?? 80));
    setText($("fr-key"), s.key_file || "miner.key");
    setText($("fr-key-msg"), "");
    renderDevices();
    const dialog = $("first-run");
    if (!dialog.open) dialog.showModal();
    show(0);
  }

  async function reveal() {
    if (host.isPreview) {
      setText($("fr-key-msg"), "Preview: the Hub app opens the folder that holds this file.");
      return;
    }
    try {
      await host.call("reveal_key_file", { keyFile: host.settings().key_file });
      setText($("fr-key-msg"), "");
    } catch (err) {
      setText($("fr-key-msg"), `The folder could not be opened: ${err}`);
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(host.settings().key_file || "");
      setText($("fr-key-msg"), "Path copied.");
    } catch {
      setText($("fr-key-msg"), "Copying was refused here; select the path above instead.");
    }
  }

  function finish() {
    const devices = picked();
    if (!NetStatus.sheetReady({ backup: $("fr-backup").checked, devices })) return refresh();
    markDone();
    $("first-run").close();
    host.onFinish(devices);
  }

  function init(options) {
    host = options;
    $("fr-next").addEventListener("click", () => show(step + 1));
    $("fr-back").addEventListener("click", () => show(step - 1));
    $("fr-close").addEventListener("click", () => $("first-run").close());
    $("fr-open").addEventListener("click", reveal);
    $("fr-copy").addEventListener("click", copy);
    $("fr-backup").addEventListener("change", refresh);
    $("fr-devices").addEventListener("change", refresh);
    $("fr-start").addEventListener("click", finish);
  }

  return { init, open, done, refresh: () => host && $("first-run").open && refresh() };
})();
