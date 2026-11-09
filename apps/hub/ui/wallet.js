// Wallet page: watch-only balances (ADR-083). The Hub holds no keys
// (ADR-045); sending and receiving stay in the Maya2C Wallet app.
// Everything the user or the node supplies is drawn with textContent only.
"use strict";

const Wallet = (() => {
  const KEY = "hub.watched";
  const MAX = 20; // watch.rs MAX_WATCHED
  const POLL_MS = 15000;
  const DECIMALS = 24n; // ADR-068
  let ctx = null;
  let rows = [];

  const load = () => {
    try {
      const saved = JSON.parse(localStorage.getItem(KEY) || "[]");
      return Array.isArray(saved) ? saved.filter((a) => typeof a === "string").slice(0, MAX) : [];
    } catch {
      return [];
    }
  };
  const save = (list) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(list));
    } catch {
      // Private window or blocked storage: the list lasts this session only.
    }
  };
  let watched = load();

  // Exact: a 256-bit decimal string never passes through a float.
  function formatMaya(base) {
    if (typeof base !== "string" || !/^\d+$/.test(base)) return "—";
    const unit = 10n ** DECIMALS;
    const value = BigInt(base);
    const whole = (value / unit).toLocaleString();
    const frac = (value % unit).toString().padStart(Number(DECIMALS), "0").slice(0, 6).replace(/0+$/, "");
    // Dust below the six shown decimals is not nothing; say so.
    if (value > 0n && value < unit / 1000000n) return "<0.000001";
    return frac ? `${whole}.${frac}` : whole;
  }

  function el(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text != null) node.textContent = text;
    return node;
  }

  function render() {
    const list = document.getElementById("w-list");
    if (!list) return;
    const total = rows.reduce((sum, r) => (r.balance ? sum + BigInt(r.balance) : sum), 0n);
    document.getElementById("w-total").textContent = rows.some((r) => r.balance) ? formatMaya(total.toString()) : "—";
    if (!watched.length) {
      list.replaceChildren(el("p", "fine", "No addresses yet. Paste one of yours above: it is only read, never used to sign."));
      return;
    }
    list.replaceChildren(
      ...watched.map((input, i) => {
        const r = rows.find((x) => x.input === input) || { input };
        const row = el("div", "watch-row");
        const who = el("div", "watch-addr");
        who.append(el("strong", null, r.address_text || r.hex || input));
        if (r.error) who.append(el("span", "fine warn-text", r.error));
        else if (r.nonce != null) who.append(el("span", "fine", `${r.nonce} sent`));
        const amount = el("div", "watch-bal", r.balance ? `${formatMaya(r.balance)} MAYA` : "—");
        const remove = el("button", "ghost", "Remove");
        remove.title = "Stop watching this address";
        remove.addEventListener("click", () => {
          watched = watched.filter((_, j) => j !== i);
          save(watched);
          refresh();
        });
        row.append(who, amount, remove);
        return row;
      }),
    );
  }

  async function refresh() {
    if (!ctx) return;
    if (!watched.length) {
      rows = [];
      render();
      return;
    }
    try {
      rows = await ctx.call("watch_balances", { url: ctx.url(), addresses: watched });
      document.getElementById("w-msg").textContent = "";
    } catch (e) {
      document.getElementById("w-msg").textContent = `The node did not answer: ${e}`;
    }
    render();
  }

  function add() {
    const input = document.getElementById("w-input");
    const value = input.value.trim();
    const msg = document.getElementById("w-msg");
    if (!value) return;
    if (watched.includes(value)) {
      msg.textContent = "Already watched.";
      return;
    }
    if (watched.length >= MAX) {
      msg.textContent = `At most ${MAX} addresses.`;
      return;
    }
    watched = [...watched, value];
    save(watched);
    input.value = "";
    refresh();
  }

  function init(context) {
    ctx = context;
    document.getElementById("w-add").addEventListener("click", add);
    document.getElementById("w-input").addEventListener("keydown", (e) => {
      if (e.key === "Enter") add();
    });
    render();
    refresh();
    setInterval(refresh, POLL_MS);
  }

  return { init, formatMaya };
})();
