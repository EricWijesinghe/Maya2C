// Maya Chat window. Everything privileged is a Rust command; this file only
// calls them and draws the results.
const invoke = window.__TAURI__.core.invoke;
const POLL_MS = 5000;
const $ = (id) => document.getElementById(id);

const state = { address: null, contacts: [], peer: null, polling: null };

function short(addr) {
  return `${addr.slice(0, 8)}…${addr.slice(-6)}`;
}

function nameOf(addr) {
  const c = state.contacts.find((c) => c.address === addr);
  return c ? c.name : short(addr);
}

function say(id, text) {
  $(id).textContent = text || "";
}

function drawContacts() {
  const nav = $("contacts");
  nav.replaceChildren();
  if (state.contacts.length === 0) {
    const p = document.createElement("p");
    p.className = "hint";
    p.textContent = "No contacts yet. Add someone by their address.";
    nav.append(p);
  }
  for (const c of state.contacts) {
    const b = document.createElement("button");
    b.className = "contact" + (c.address === state.peer ? " active" : "");
    b.textContent = c.name;
    b.title = c.address;
    b.onclick = () => openPeer(c.address);
    nav.append(b);
  }
}

function drawLine(line) {
  const li = document.createElement("li");
  li.className = line.mine ? "mine" : "theirs";
  const text = document.createElement("span");
  text.textContent = line.text;
  const at = document.createElement("time");
  at.textContent = new Date(line.at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  li.append(text, at);
  $("lines").append(li);
  $("lines").scrollTop = $("lines").scrollHeight;
}

async function openPeer(addr) {
  state.peer = addr;
  say("peer", nameOf(addr));
  $("text").disabled = false;
  $("send").disabled = false;
  drawContacts();
  $("lines").replaceChildren();
  try {
    for (const line of await invoke("history", { peer: addr })) drawLine(line);
  } catch (e) {
    say("chat-error", e);
  }
  $("text").focus();
}

async function poll() {
  try {
    const fresh = await invoke("poll");
    say("side-status", `Connected · checked ${new Date().toLocaleTimeString()}`);
    for (const line of fresh) {
      if (!state.contacts.some((c) => c.address === line.peer)) {
        state.contacts = await invoke("add_contact", { name: short(line.peer), addr: line.peer });
        drawContacts();
      }
      if (line.peer === state.peer) drawLine(line);
      else say("side-status", `New message from ${nameOf(line.peer)}`);
    }
  } catch (e) {
    say("side-status", e);
  }
}

function startPolling() {
  if (state.polling) return;
  poll();
  state.polling = setInterval(poll, POLL_MS);
}

async function enter(status) {
  state.address = status.address;
  state.contacts = status.contacts;
  $("welcome").hidden = true;
  $("app").hidden = false;
  $("copy").textContent = short(status.address);
  $("copy").title = status.address;
  $("relay").value = status.relay;
  drawContacts();
  if (status.relay) {
    try {
      await invoke("publish");
      startPolling();
    } catch (e) {
      say("side-status", e);
    }
  } else {
    say("side-status", "Set a relay in Settings to start.");
  }
}

$("create").onclick = async () => {
  $("create").disabled = true;
  try {
    await invoke("create_identity");
    await enter(await invoke("status"));
  } catch (e) {
    say("welcome-error", e);
    $("create").disabled = false;
  }
};

$("copy").onclick = async () => {
  await navigator.clipboard.writeText(state.address);
  say("side-status", "Address copied. Share it with someone to chat.");
};

$("add").onsubmit = async (ev) => {
  ev.preventDefault();
  try {
    state.contacts = await invoke("add_contact", { name: $("add-name").value, addr: $("add-address").value });
    $("add").reset();
    drawContacts();
  } catch (e) {
    say("side-status", e);
  }
};

$("save-relay").onclick = async () => {
  try {
    await invoke("set_relay", { relay: $("relay").value });
    await invoke("publish");
    say("side-status", "Relay saved and your key published.");
    startPolling();
  } catch (e) {
    say("side-status", e);
  }
};

$("compose").onsubmit = async (ev) => {
  ev.preventDefault();
  const text = $("text").value.trim();
  if (!text || !state.peer) return;
  $("send").disabled = true;
  say("chat-error", "");
  try {
    drawLine(await invoke("send", { peer: state.peer, text }));
    $("text").value = "";
  } catch (e) {
    say("chat-error", e);
  } finally {
    $("send").disabled = false;
    $("text").focus();
  }
};

(async () => {
  try {
    const status = await invoke("status");
    if (status.ready) await enter(status);
    else $("welcome").hidden = false;
  } catch (e) {
    $("welcome").hidden = false;
    say("welcome-error", e);
  }
})();
