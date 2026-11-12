// Maya Chat window. Everything privileged is a Rust command; this file only
// calls them and draws the results.
const invoke = window.__TAURI__.core.invoke;
const POLL_MS = 5000;
// During first run the reply is the point, so the mailbox is checked often.
const FIRST_RUN_POLL_MS = 1000;
const FIRST_RUN_WAIT_MS = 45000;
const BOT_NAME = "Maya (welcome bot)";
const $ = (id) => document.getElementById(id);

const state = { address: null, contacts: [], peer: null, polling: null, replyTo: null };
// The reactions offered (maya_chat::frame::REACTION_EMOJI), with the names a
// screen reader says.
const REACTIONS = [
  ["👍", "thumbs up"],
  ["❤️", "heart"],
  ["😂", "laughing"],
  ["😮", "surprised"],
  ["😢", "sad"],
  ["🙏", "thanks"],
  ["🔥", "fire"],
  ["🎉", "celebrate"],
];
const LONG_PRESS_MS = 500;
// Installed custom emoji the reaction picker shows after the fixed eight.
const CUSTOM_IN_PICKER = 8;
// How often lines past their disappearing deadline are removed.
const EXPIRE_MS = 1000;

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

function reactionName(emoji) {
  const known = REACTIONS.find(([e]) => e === emoji);
  return known ? known[1] : "custom emoji";
}

// The quote above a reply: this device's own copy of the original, a plain
// notice when it does not hold one, and nothing when the hash did not match.
function quoteOf(line) {
  if (!line.quote || line.quote.state === "mismatch") return null;
  const q = document.createElement("div");
  q.className = "quote";
  if (line.quote.state === "shown") {
    q.textContent = line.quote.text;
  } else {
    q.classList.add("missing");
    q.textContent = "original not available";
  }
  return q;
}

// One chip per emoji that is on, counted over both sides; pressing a chip
// toggles this user's own reaction.
function chipsOf(line) {
  const counts = new Map();
  for (const m of line.reactions || []) {
    if (!m.on) continue;
    const c = counts.get(m.emoji) || { n: 0, mine: false };
    counts.set(m.emoji, { n: c.n + 1, mine: c.mine || m.mine });
  }
  if (counts.size === 0) return null;
  const row = document.createElement("div");
  row.className = "reactions";
  row.setAttribute("role", "group");
  row.setAttribute("aria-label", "Reactions");
  for (const [emoji, c] of counts) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "chip" + (c.mine ? " is-mine" : "");
    chip.append(Stickers.glyph(emoji, 18), ` ${c.n}`);
    chip.setAttribute("aria-pressed", String(c.mine));
    chip.setAttribute("aria-label", `${reactionName(emoji)}, ${c.n}${c.mine ? ", including yours" : ""}. Press to ${c.mine ? "remove" : "add"} yours`);
    chip.onclick = () => react(line, emoji, !c.mine);
    row.append(chip);
  }
  return row;
}

function renderLine(line) {
  const li = document.createElement("li");
  li.className = line.mine ? "mine" : "theirs";
  // The app's own words (a timer change): never a bubble, never reactable.
  if (line.notice) {
    li.className = "notice";
    li.textContent = line.text;
    return li;
  }
  const quote = quoteOf(line);
  if (quote) li.append(quote);
  let text;
  if (line.sticker) {
    li.classList.add("is-sticker");
    text = Stickers.line(line, line.peer);
  } else {
    text = document.createElement("span");
    text.append(Stickers.inline(line.text));
  }
  const at = document.createElement("time");
  const clock = new Date(line.at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  // What happened to the words: sealed here, or opened here after the trip.
  at.textContent = `${line.mine ? "sealed" : "opened"} · ${clock}${line.timer ? " · disappears" : ""}`;
  li.append(text, at);
  const chips = chipsOf(line);
  if (chips) li.append(chips);
  if (line.outbox) li.dataset.outbox = line.outbox;
  // Lines saved before message ids existed cannot be reacted or replied to.
  if (line.id) {
    li.dataset.id = line.id;
    li.tabIndex = 0;
    li.setAttribute("aria-haspopup", "menu");
    li.title = "Right-click, long-press or press Enter to react or reply";
    li._line = line;
    li.oncontextmenu = (ev) => {
      ev.preventDefault();
      openPicker(li);
    };
    li.onkeydown = (ev) => {
      if (ev.target !== li) return;
      if (ev.key === "Enter" || ev.key === "ContextMenu" || (ev.key === "F10" && ev.shiftKey)) {
        ev.preventDefault();
        openPicker(li);
      }
    };
    let press = null;
    const cancel = () => clearTimeout(press);
    li.onpointerdown = (ev) => {
      if (ev.pointerType === "mouse") return;
      press = setTimeout(() => openPicker(li), LONG_PRESS_MS);
    };
    li.onpointerup = cancel;
    li.onpointerleave = cancel;
    li.onpointermove = cancel;
  }
  return li;
}

function drawLine(line) {
  $("lines").append(renderLine(line));
  $("lines").scrollTop = $("lines").scrollHeight;
}

// Redraws a line already on screen (a reaction changed it), or adds it.
function upsertLine(line) {
  const old = line.id && $("lines").querySelector(`li[data-id="${line.id}"]`);
  if (!old) return drawLine(line);
  const fresh = renderLine(line);
  const hadFocus = old.contains(document.activeElement);
  old.replaceWith(fresh);
  if (hadFocus) fresh.focus();
  refreshDeliveries();
}

// The picker: the reactions plus Reply, for one line.
function openPicker(li) {
  const picker = $("react-picker");
  const line = li._line;
  const mineOn = new Set((line.reactions || []).filter((m) => m.mine && m.on).map((m) => m.emoji));
  picker.replaceChildren();
  for (const [emoji, name] of REACTIONS) {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitemcheckbox");
    b.setAttribute("aria-checked", String(mineOn.has(emoji)));
    b.setAttribute("aria-label", `React with ${name}`);
    b.textContent = emoji;
    b.onclick = () => {
      closePicker();
      react(line, emoji, !mineOn.has(emoji));
    };
    picker.append(b);
  }
  // Installed custom emoji react by their 64-hex id (frame::check_emoji).
  for (const item of Stickers.customEmoji().slice(0, CUSTOM_IN_PICKER)) {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitemcheckbox");
    b.setAttribute("aria-checked", String(mineOn.has(item.id)));
    b.setAttribute("aria-label", `React with :${item.emoji}:`);
    b.append(Stickers.glyph(item.id, 22));
    b.onclick = () => {
      closePicker();
      react(line, item.id, !mineOn.has(item.id));
    };
    picker.append(b);
  }
  const reply = document.createElement("button");
  reply.type = "button";
  reply.className = "reply-action";
  reply.setAttribute("role", "menuitem");
  reply.textContent = "Reply";
  reply.onclick = () => {
    closePicker();
    startReply(line);
  };
  picker.append(reply);
  picker._from = li;
  picker.hidden = false;
  const box = li.getBoundingClientRect();
  const room = $("lines").getBoundingClientRect();
  picker.style.top = `${Math.max(room.top, box.top - picker.offsetHeight - 6)}px`;
  picker.style.left = `${Math.min(Math.max(8, box.left), window.innerWidth - picker.offsetWidth - 8)}px`;
  picker.querySelector("button").focus();
}

function closePicker() {
  const picker = $("react-picker");
  if (picker.hidden) return;
  picker.hidden = true;
  picker._from?.focus();
}

$("react-picker").onkeydown = (ev) => {
  const items = [...$("react-picker").querySelectorAll("button")];
  const i = items.indexOf(document.activeElement);
  if (ev.key === "Escape") closePicker();
  else if (ev.key === "ArrowRight" || ev.key === "ArrowDown") items[(i + 1) % items.length].focus();
  else if (ev.key === "ArrowLeft" || ev.key === "ArrowUp") items[(i - 1 + items.length) % items.length].focus();
  else return;
  ev.preventDefault();
};
document.addEventListener("pointerdown", (ev) => {
  if (!$("react-picker").contains(ev.target)) closePicker();
});

async function react(line, emoji, on) {
  say("chat-error", "");
  try {
    upsertLine(await invoke("react", { peer: line.peer, target: line.id, emoji, on }));
  } catch (e) {
    say("chat-error", e);
  }
}

function startReply(line) {
  state.replyTo = line;
  $("reply-text").textContent = line.text;
  $("reply-bar").hidden = false;
  $("text").focus();
}

function cancelReply() {
  state.replyTo = null;
  $("reply-bar").hidden = true;
}

$("reply-cancel").onclick = () => {
  cancelReply();
  $("text").focus();
};

const DELIVERY_LABEL = {
  sending: "Sending",
  waiting: "Waiting for receipt",
  delivered: "Delivered",
  expired: "Expired, no receipt",
};

// Shows a sent message's state; an expired one gets a Dismiss button.
function paintDelivery(li, d) {
  let tag = li.querySelector(".delivery");
  if (!tag) {
    tag = document.createElement("span");
    tag.className = "delivery";
    li.append(tag);
  }
  tag.dataset.state = d.state;
  tag.textContent = DELIVERY_LABEL[d.state];
  let dismiss = li.querySelector(".dismiss");
  if (d.state === "expired" && !dismiss) {
    dismiss = document.createElement("button");
    dismiss.className = "dismiss";
    dismiss.textContent = "Dismiss";
    dismiss.onclick = () => dismissDelivery(li, d.id);
    li.append(dismiss);
  } else if (d.state !== "expired" && dismiss) {
    dismiss.remove();
  }
}

async function dismissDelivery(li, id) {
  try {
    await invoke("dismiss_delivery", { id });
    delete li.dataset.outbox;
    li.querySelector(".delivery")?.remove();
    li.querySelector(".dismiss")?.remove();
  } catch (e) {
    say("chat-error", e);
  }
}

async function refreshDeliveries() {
  if (!state.peer) return;
  try {
    for (const d of await invoke("deliveries", { peer: state.peer })) {
      const li = $("lines").querySelector(`li[data-outbox="${d.id}"]`);
      if (li) paintDelivery(li, d);
    }
  } catch (e) {
    say("side-status", e);
  }
}

// The chat's disappearing-message setting, the plain limit while it is on,
// the old-app warning and the one-time suggestion.
async function drawTimer() {
  if (!state.peer) return;
  const view = await invoke("chat_timer", { peer: state.peer });
  const select = $("timer");
  select.replaceChildren(
    ...view.options.map(([secs, label]) => {
      const o = document.createElement("option");
      o.value = String(secs);
      o.textContent = label;
      o.selected = secs === view.secs;
      return o;
    }),
  );
  $("timer-limit").textContent = view.limit;
  $("timer-limit").hidden = view.secs === 0;
  $("timer-warn").textContent = view.may_not_support || "";
  $("timer-warn").hidden = !view.may_not_support;
  $("timer-prompt-text").textContent = view.prompt || "";
  $("timer-prompt").hidden = !view.prompt;
  $("timer-bar").hidden = false;
}

$("timer").onchange = async () => {
  if (!state.peer) return;
  say("chat-error", "");
  try {
    drawLine(await invoke("set_timer", { peer: state.peer, secs: Number($("timer").value) }));
  } catch (e) {
    say("chat-error", e);
  }
  await drawTimer().catch((e) => say("chat-error", e));
};

$("timer-prompt-no").onclick = async () => {
  await invoke("dismiss_timer_prompt").catch((e) => say("chat-error", e));
  $("timer-prompt").hidden = true;
};

// Removes lines whose deadline has passed, from the screen and the store.
async function expire() {
  try {
    for (const id of await invoke("expire")) {
      $("lines").querySelector(`li[data-id="${id}"]`)?.remove();
      if (state.replyTo && state.replyTo.id === id) cancelReply();
    }
  } catch (e) {
    say("side-status", e);
  }
}

async function openPeer(addr) {
  state.peer = addr;
  cancelReply();
  $("peer").replaceChildren(avatar(addr), document.createTextNode(nameOf(addr)));
  $("text").disabled = false;
  $("send").disabled = false;
  $("sticker-toggle").disabled = false;
  drawContacts();
  $("lines").replaceChildren();
  try {
    for (const line of await invoke("history", { peer: addr })) drawLine(line);
    await refreshDeliveries();
    await drawTimer();
    await Profiles.open(addr);
  } catch (e) {
    say("chat-error", e);
  }
  $("text").focus();
}

async function poll() {
  try {
    const fresh = await invoke("poll", { open: state.peer });
    say("side-status", `Connected · checked ${new Date().toLocaleTimeString()}`);
    for (const line of fresh) {
      if (!state.contacts.some((c) => c.address === line.peer)) {
        state.contacts = await invoke("add_contact", { name: short(line.peer), addr: line.peer });
        drawContacts();
      }
      // A pack someone sent finished arriving: the picker shows it now.
      if (line.text.startsWith("Sticker pack ") && !line.sticker) Stickers.refresh().catch(() => {});
      if (line.peer === state.peer) upsertLine(line);
      else say("side-status", `New message from ${nameOf(line.peer)}`);
      if (line.notice && line.peer === state.peer) await drawTimer();
    }
    await refreshDeliveries();
    // A card arrives without a line; redraw the open chat's.
    await Profiles.refresh();
  } catch (e) {
    say("side-status", e);
  }
}

function startPolling() {
  if (state.polling) return;
  poll();
  state.polling = setInterval(poll, POLL_MS);
}

setInterval(() => {
  if (!$("app").hidden) expire();
}, EXPIRE_MS);

async function enter(status, open) {
  state.address = status.address;
  state.contacts = status.contacts;
  $("welcome").hidden = true;
  $("app").hidden = false;
  $("copy").textContent = short(status.address);
  $("copy").title = status.address;
  $("relay").value = status.relay;
  drawContacts();
  Stickers.init(async (pack, id) => {
    if (!state.peer) return;
    say("chat-error", "");
    try {
      drawLine(await invoke("send_sticker", { peer: state.peer, pack, id }));
      await refreshDeliveries();
    } catch (e) {
      say("chat-error", e);
    }
  }).catch((e) => say("side-status", e));
  $("text").oninput = () => Stickers.suggest($("text").value);
  Profiles.init((e) => say("chat-error", e)).catch((e) => say("side-status", e));
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
    const target = state.replyTo;
    drawLine(
      target
        ? await invoke("reply", { peer: state.peer, target: target.id, text })
        : await invoke("send", { peer: state.peer, text }),
    );
    cancelReply();
    await refreshDeliveries();
    $("text").value = "";
    Stickers.suggest("");
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
