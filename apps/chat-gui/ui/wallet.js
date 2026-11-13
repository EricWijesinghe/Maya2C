// In-chat wallet bubbles and the mini-app runner (ADR-088, ADR-138).
// Chat never signs a payment: a Pay button shows the confirm text, then hands
// the user a wallet link. A mini-app runs in a sandboxed frame; its only way
// out is postMessage, relayed to the Rust bridge with the frame's token.
const Wallet = (() => {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  // Windows serves custom schemes as http://<scheme>.localhost.
  const APP_URL = navigator.userAgent.includes("Windows") ? "http://miniapp.localhost/" : "miniapp://localhost/";
  const run = { token: null, frame: null, peer: null };

  function node(tag, cls, text) {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  }

  // Resolves true only on the confirm button: closing the dialog is a no.
  function confirmText(text) {
    return new Promise((resolve) => {
      const d = el("confirm-dialog");
      el("confirm-text").textContent = text;
      const done = (yes) => () => { d.close(); resolve(yes); };
      el("confirm-yes").onclick = done(true);
      el("confirm-no").onclick = done(false);
      d.onclose = () => resolve(false);
      d.showModal();
    });
  }

  function showLink(link) {
    el("link-text").value = link;
    el("link-dialog").showModal();
  }

  function bubble(line) {
    const p = line.pay;
    const box = node("span", `pay pay-${p.kind}`);
    box.append(node("strong", null, line.text));
    if (p.memo) box.append(node("small", null, p.memo));
    if (p.tx) box.append(node("small", "mono", `tx ${p.tx.slice(0, 16)}…`));
    if (p.kind === "request" && p.link && p.confirm) {
      const pay = node("button", "primary", "Pay");
      // shortcut: the wallet link is shown to copy; opening it directly needs tauri-plugin-opener.
      pay.onclick = async () => { if (await confirmText(p.confirm)) showLink(p.link); };
      const paid = node("button", "secondary", "I paid");
      paid.onclick = () => receipt(line.peer);
      box.append(pay, paid);
    }
    return box;
  }

  async function receipt(peer) {
    const link = window.prompt("Paste the link your wallet gave you after paying (maya2c-chat://paid?…)");
    if (!link) return;
    try { drawLine(await invoke("pay_receipt", { peer, paidLink: link })); }
    catch (e) { el("chat-error").textContent = String(e); }
  }

  async function request(peer) {
    const d = el("request-dialog");
    el("request-error").textContent = "";
    d.showModal();
    el("request-send").onclick = async () => {
      try {
        drawLine(await invoke("pay_request", { peer, amount: el("request-amount").value, memo: el("request-memo").value }));
        d.close();
      } catch (e) {
        el("request-error").textContent = String(e);
        if (/set up payments|set the node/.test(String(e))) await settings();
      }
    };
  }

  // shortcut: three prompts; a settings form once the Pay card is designed.
  async function settings() {
    const chain = window.prompt("Chain genesis id (64 hex characters, from your wallet's network screen)");
    if (!chain) return;
    const node = window.prompt("Node RPC URL for the chain tip (one you run)");
    if (!node) return;
    const binding = window.prompt("Paste the binding from your wallet's 'Chat payment binding'");
    if (!binding) return;
    try { await invoke("pay_settings", { chain, node, binding }); el("request-error").textContent = "Payments set up. Send the request again."; }
    catch (e) { el("request-error").textContent = String(e); }
  }

  function post(reply) {
    // The frame is an opaque origin, so "*" is the only target; replies go only to it.
    if (run.frame) run.frame.contentWindow.postMessage(reply, "*");
  }

  // Follows the bridge's replies until one goes back to the app.
  async function settle(reply) {
    while (reply && reply.reply === "ask") {
      const yes = await confirmText(reply.text);
      reply = await invoke("miniapp_answer", { token: run.token, prompt: reply.prompt, yes });
    }
    if (!reply) return;
    if (reply.reply === "open-wallet") { showLink(reply.link); post({ reply: "confirmed" }); return; }
    if (reply.reply === "send") {
      try { drawLine(await invoke("send", { peer: run.peer, text: reply.text })); post({ reply: "sent" }); }
      catch (e) { post({ reply: "refused", why: String(e) }); }
      return;
    }
    post(reply);
  }

  window.addEventListener("message", async (event) => {
    // Unknown origin: anything not from the frame we started is dropped.
    if (!run.frame || event.source !== run.frame.contentWindow) return;
    try { await settle(await invoke("miniapp_call", { token: run.token, call: event.data })); }
    catch (e) { post({ reply: "refused", why: "malformed call" }); }
  });

  async function open(id, peer) {
    run.token = await invoke("miniapp_open", { id });
    run.peer = peer;
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.setAttribute("allow", "");
    frame.className = "miniapp-frame";
    frame.src = APP_URL;
    el("apps-frame").replaceChildren(frame);
    run.frame = frame;
  }

  async function close() {
    run.frame = null;
    run.token = null;
    el("apps-frame").replaceChildren();
    await invoke("miniapp_close");
  }

  async function apps(peer) {
    const list = el("apps-list");
    list.replaceChildren();
    for (const a of await invoke("miniapps")) {
      const b = node("button", "secondary", a.name || a.id);
      if (a.error) { b.disabled = true; b.title = a.error; }
      else b.title = `Publisher ${a.publisher}\nAsks for: ${a.permissions.join(", ") || "nothing"}`;
      b.onclick = () => open(a.id, peer).catch((e) => { el("apps-error").textContent = String(e); });
      list.append(b);
    }
    if (!list.children.length) list.append(node("p", "hint", "No mini-apps installed."));
    el("apps-dialog").onclose = close;
    el("apps-dialog").showModal();
  }

  el("link-copy").onclick = () => navigator.clipboard.writeText(el("link-text").value);
  for (const b of document.querySelectorAll(".dialog-close")) b.onclick = () => b.closest("dialog").close();
  return { bubble, request, apps };
})();
