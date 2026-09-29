// Maya Chat window. Everything privileged is a Rust command; this file only
// calls them and draws the results.
const invoke = window.__TAURI__.core.invoke;
const POLL_MS = 5000;
// During first run the reply is the point, so the mailbox is checked often.
const FIRST_RUN_POLL_MS = 1000;
const FIRST_RUN_WAIT_MS = 45000;
const BOT_NAME = "Maya (welcome bot)";
const $ = (id) => document.getElementById(id);

const state = { address: null, contacts: [], peer: null, polling: null };

function short(addr) {
  return `${addr.slice(0, 8)}…${addr.slice(-6)}`;
}

function nameOf(addr) {
  const c = state.contacts.find((c) => c.address === addr);
  return c ? c.name : short(addr);
}

// A colour pair from an address, so a contact is recognisable at a glance
// and two contacts with the same name are not.
function avatar(addr) {
  const span = document.createElement("span");
  span.className = "avatar";
  const a = parseInt(addr.slice(0, 6), 16) % 360;
  const b = parseInt(addr.slice(6, 12), 16) % 360;
  span.style.background = `linear-gradient(135deg, hsl(${a} 85% 60%), hsl(${b} 80% 55%))`;
  return span;
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
    b.append(avatar(c.address), document.createTextNode(c.name));
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
  const clock = new Date(line.at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  // What happened to the words: sealed here, or opened here after the trip.
  at.textContent = `${line.mine ? "sealed" : "opened"} · ${clock}`;
  li.append(text, at);
  $("lines").append(li);
  $("lines").scrollTop = $("lines").scrollHeight;
}

async function openPeer(addr) {
  state.peer = addr;
  $("peer").replaceChildren(avatar(addr), document.createTextNode(nameOf(addr)));
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

async function enter(status, open) {
  state.address = status.address;
  state.contacts = status.contacts;
  $("welcome").hidden = true;
  $("app").hidden = false;
  $("copy").textContent = short(status.address);
  $("copy").title = status.address;
  $("relay").value = status.relay;
  drawContacts();
  if (open) openPeer(open);
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

// One first-run step: shown as it starts, ticked when the Rust command it
// waits on returns, marked failed if it throws. Nothing here is a timer
// dressed up as progress.
async function step(label, work) {
  const li = document.createElement("li");
  li.textContent = label;
  $("steps").append(li);
  try {
    const detail = await work();
    li.classList.add("done");
    if (typeof detail === "string") {
      const small = document.createElement("small");
      small.textContent = detail;
      li.append(small);
    }
    return detail;
  } catch (e) {
    li.classList.add("failed");
    throw e;
  }
}

// Checks the mailbox until `bot` has answered. The lines are kept in
// history by the poll itself, so the chat opens with them in place.
async function waitForReply(bot) {
  const until = Date.now() + FIRST_RUN_WAIT_MS;
  while (Date.now() < until) {
    const fresh = await invoke("poll");
    if (fresh.some((l) => l.peer === bot)) return "Reply opened on this computer";
    await new Promise((r) => setTimeout(r, FIRST_RUN_POLL_MS));
  }
  throw new Error("No reply yet; it will appear in the chat when it arrives.");
}

async function firstRun() {
  let addr = "";
  await step("Generating your ML-DSA-65 identity", async () => {
    addr = await invoke("create_identity");
    return short(addr);
  });
  const status = await invoke("status");
  await step("Publishing your X-Wing prekey to the relay", async () => {
    await invoke("publish");
    return "Anyone with your address can now write to you";
  });
  const bot = status.welcome_bot;
  if (!bot) return null;
  await step("Sealing a hello to Maya, the welcome bot", async () => {
    await invoke("add_contact", { name: BOT_NAME, addr: bot });
    await invoke("send", { peer: bot, text: "hello" });
    return "The relay holds ciphertext only";
  });
  await step("Waiting for Maya's sealed reply", () => waitForReply(bot));
  return bot;
}

$("create").onclick = async () => {
  $("create").hidden = true;
  $("steps").hidden = false;
  say("welcome-error", "");
  let open = null;
  try {
    open = await firstRun();
  } catch (e) {
    say("welcome-error", e.message || e);
    // An identity may already exist; the app is usable, so let them in.
    try {
      if ((await invoke("status")).ready) {
        $("continue").hidden = false;
        return;
      }
    } catch (_) {
      /* status unreadable too: offer a fresh start below */
    }
    $("create").hidden = false;
    $("steps").replaceChildren();
    return;
  }
  await enter(await invoke("status"), open);
};

$("continue").onclick = async () => {
  const status = await invoke("status");
  await enter(status, status.contacts.some((c) => c.address === status.welcome_bot) ? status.welcome_bot : null);
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
