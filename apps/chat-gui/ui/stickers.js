// Stickers in the window: drawing every format, the picker, emoji
// suggestions, "Add pack" on a received sticker, and the pack maker.
// Files come from Rust as raw bytes (an ArrayBuffer), never JSON arrays.
const Stickers = (() => {
  const invoke = window.__TAURI__.core.invoke;
  const MIME = {
    lottie: "application/json", tgs: "application/gzip", webm: "video/webm",
    webp: "image/webp", png: "image/png", gif: "image/gif", svg: "image/svg+xml",
  };
  const ACCEPT = ".json,.tgs,.webm,.webp,.png,.apng,.gif,.svg";
  const urls = new Map(); // id -> object URL (images, video)
  const anims = new Map(); // id -> parsed Lottie JSON
  let packs = [];
  let recent = [];
  let onSend = async () => {};

  async function bytesOf(id) {
    return new Uint8Array(await invoke("sticker_file", { id }));
  }

  async function lottieOf(id, format) {
    if (anims.has(id)) return anims.get(id);
    let bytes = await bytesOf(id);
    if (format === "tgs") {
      // Telegram's .tgs is gzipped Lottie JSON.
      const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
      bytes = new Uint8Array(await new Response(stream).arrayBuffer());
    }
    const data = JSON.parse(new TextDecoder().decode(bytes));
    anims.set(id, data);
    return data;
  }

  async function urlOf(id, format) {
    if (!urls.has(id)) urls.set(id, URL.createObjectURL(new Blob([await bytesOf(id)], { type: MIME[format] })));
    return urls.get(id);
  }

  // ---- motion ------------------------------------------------------------
  // Everything that moves stops under the OS reduced-motion setting (showing
  // its first frame) and while the window is not focused (where it stands).
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
  const players = new Set(); // { box, play(), pause() }
  const still = () => reduced.matches || !document.hasFocus();

  function sync() {
    for (const p of players) {
      if (!p.box.isConnected && p.seen) { players.delete(p); continue; }
      p.seen = p.seen || p.box.isConnected;
      if (still()) p.pause(reduced.matches); else p.play();
    }
  }
  reduced.addEventListener("change", sync);
  window.addEventListener("blur", sync);
  window.addEventListener("focus", sync);

  function track(player) {
    players.add(player);
    if (still()) player.pause(reduced.matches); else player.play();
  }

  // An image format that may animate (GIF, APNG, WebP, SVG) cannot be paused,
  // so a still copy of its first frame, drawn on load, stands in for it.
  function imageOf(box, ref, url) {
    const img = document.createElement("img");
    const frozen = document.createElement("canvas");
    img.alt = ref.emoji || "sticker";
    frozen.hidden = true;
    frozen.setAttribute("aria-hidden", "true");
    img.onload = () => {
      frozen.width = img.naturalWidth || box.clientWidth;
      frozen.height = img.naturalHeight || box.clientHeight;
      frozen.getContext("2d").drawImage(img, 0, 0, frozen.width, frozen.height);
      track({
        box,
        play() { img.hidden = false; frozen.hidden = true; },
        pause() { img.hidden = true; frozen.hidden = false; },
      });
    };
    img.src = url;
    box.append(img, frozen);
  }

  // Draws one sticker into a new element of the given pixel size.
  function render(ref, size) {
    const box = document.createElement("div");
    box.className = "sticker";
    box.style.width = box.style.height = `${size}px`;
    box.setAttribute("role", "img");
    box.setAttribute("aria-label", ref.emoji || "sticker");
    (async () => {
      try {
        if (ref.format === "lottie" || ref.format === "tgs") {
          const anim = window.lottie.loadAnimation({
            container: box, renderer: "svg", loop: true, autoplay: false,
            animationData: await lottieOf(ref.id, ref.format),
          });
          track({
            box,
            play: () => anim.play(),
            pause: (first) => (first ? anim.goToAndStop(0, true) : anim.pause()),
          });
        } else if (ref.format === "webm") {
          const v = document.createElement("video");
          Object.assign(v, { autoplay: false, loop: true, muted: true, playsInline: true });
          v.src = await urlOf(ref.id, ref.format);
          box.append(v);
          track({
            box,
            play: () => { v.play().catch(() => {}); },
            pause: (first) => { v.pause(); if (first) v.currentTime = 0; },
          });
        } else {
          imageOf(box, ref, await urlOf(ref.id, ref.format));
        }
      } catch (e) {
        box.textContent = ref.emoji || "🖼️";
        box.title = `This sticker could not be shown: ${e.message || e}`;
      }
    })();
    return box;
  }

  function findItem(id) {
    for (const p of packs) {
      const item = p.items.find((i) => i.id === id);
      if (item) return { pack: p, item };
    }
    return null;
  }

  async function refresh() {
    ({ packs, recent } = await invoke("sticker_packs"));
    drawTabs();
  }

  // ---- picker ----------------------------------------------------------
  let tab = "recent";

  function drawTabs() {
    const tabs = document.getElementById("sticker-tabs");
    tabs.replaceChildren();
    const add = (key, label, child) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "tab" + (tab === key ? " active" : "");
      b.title = label;
      b.append(child);
      b.onclick = () => { tab = key; drawTabs(); };
      tabs.append(b);
    };
    add("recent", "Recent", document.createTextNode("🕘"));
    for (const p of packs) add(p.id, p.title, render({ ...p.items[0] }, 28));
    add("manage", "Make or manage packs", document.createTextNode("＋"));
    drawGrid();
  }

  function drawGrid() {
    const grid = document.getElementById("sticker-grid");
    grid.replaceChildren();
    if (tab === "manage") return drawManage(grid);
    const items = tab === "recent"
      ? recent.map(findItem).filter(Boolean).map((f) => ({ ...f.item, pack: f.pack.id }))
      : (packs.find((p) => p.id === tab)?.items || []).map((i) => ({ ...i, pack: tab, kind: packs.find((p) => p.id === tab).kind }));
    if (items.length === 0) {
      const p = document.createElement("p");
      p.className = "hint";
      p.textContent = tab === "recent" ? "Stickers you send appear here." : "This pack is empty.";
      grid.append(p);
    }
    for (const item of items) grid.append(pickButton(item, 88));
  }

  function pickButton(item, size) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "pick";
    b.title = item.emoji;
    b.append(render(item, size));
    // A custom emoji goes into the text as its shortcode; sending turns it
    // into `:<id>:` (maya_chat::emoji::expand_codes).
    b.onclick = item.kind === "emoji" ? () => insertCode(item.emoji) : () => send(item.pack, item.id);
    return b;
  }

  function insertCode(code) {
    const box = document.getElementById("text");
    const at = box.selectionStart ?? box.value.length;
    box.value = `${box.value.slice(0, at)}:${code}:${box.value.slice(box.selectionEnd ?? at)}`;
    box.focus();
  }

  // ---- custom emoji ------------------------------------------------------
  // Mirrors maya_chat::emoji::{resolve, segments}: a 64-hex id from an
  // installed emoji pack draws its file, any other id draws FALLBACK.
  const HEX_ID = /^[0-9a-f]{64}$/;
  const INLINE = /:([0-9a-f]{64}):/g;
  const FALLBACK = "◇";

  function emojiItem(id) {
    for (const p of packs) {
      if (p.kind !== "emoji") continue;
      const item = p.items.find((i) => i.id === id);
      if (item) return item;
    }
    return null;
  }

  function customEmoji() {
    return packs.filter((p) => p.kind === "emoji").flatMap((p) => p.items);
  }

  // One reaction value or inline id, as an element.
  function glyph(value, size = 20) {
    if (!HEX_ID.test(value)) return document.createTextNode(value);
    const item = emojiItem(value);
    if (!item) {
      const s = document.createElement("span");
      s.className = "emoji-fallback";
      s.textContent = FALLBACK;
      s.setAttribute("role", "img");
      s.setAttribute("aria-label", "custom emoji not on this computer");
      s.title = "A custom emoji this computer does not have";
      return s;
    }
    const box = render({ ...item, emoji: `:${item.emoji}:` }, size);
    box.classList.add("emoji-inline");
    box.title = `:${item.emoji}:`;
    return box;
  }

  // A line of text with its `:<id>:` custom emoji drawn inline.
  function inline(text) {
    const frag = document.createDocumentFragment();
    let last = 0;
    for (const m of text.matchAll(INLINE)) {
      frag.append(text.slice(last, m.index), glyph(m[1]));
      last = m.index + m[0].length;
    }
    frag.append(text.slice(last));
    return frag;
  }

  async function send(pack, id) {
    await onSend(pack, id);
    recent = [id, ...recent.filter((r) => r !== id)].slice(0, 30);
    if (tab === "recent") drawGrid();
  }

  function drawManage(grid) {
    const make = document.createElement("button");
    make.type = "button";
    make.className = "primary";
    make.textContent = "Make a sticker pack";
    make.onclick = openMaker;
    grid.append(make);
    for (const p of packs) {
      const row = document.createElement("div");
      row.className = "pack-row";
      row.append(render({ ...p.items[0] }, 40), document.createTextNode(` ${p.title} · ${p.items.length}`));
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "secondary";
      remove.textContent = "Remove";
      remove.onclick = async () => {
        try { await invoke("remove_pack", { pack: p.id }); await refresh(); } catch (e) { alertLine(e); }
      };
      row.append(remove);
      grid.append(row);
    }
  }

  // ---- emoji suggestions ------------------------------------------------
  // Typing exactly one emoji offers every installed sticker made for it.
  function suggest(text) {
    const strip = document.getElementById("sticker-suggest");
    strip.replaceChildren();
    const emoji = text.trim();
    const isEmoji = emoji && [...emoji].length <= 3 && /\p{Extended_Pictographic}/u.test(emoji);
    const hits = isEmoji
      ? packs.flatMap((p) => p.items.filter((i) => i.emoji.includes(emoji)).map((i) => ({ ...i, pack: p.id })))
      : [];
    strip.hidden = hits.length === 0;
    for (const item of hits.slice(0, 12)) {
      const b = pickButton(item, 64);
      const send0 = b.onclick;
      b.onclick = async () => { await send0(); strip.hidden = true; document.getElementById("text").value = ""; };
      strip.append(b);
    }
  }

  // ---- a received sticker's line -----------------------------------------
  function line(lineData, peer) {
    const wrap = document.createElement("div");
    wrap.className = "sticker-line";
    wrap.append(render({ ...lineData.sticker, emoji: lineData.text }, 160));
    const s = lineData.sticker;
    if (!lineData.mine && !packs.some((p) => p.id === s.pack)) {
      const add = document.createElement("button");
      add.type = "button";
      add.className = "add-pack";
      add.textContent = `Add pack “${s.title}”`;
      add.onclick = async () => {
        add.disabled = true;
        add.textContent = "Asked for the pack…";
        try { await invoke("request_pack", { peer, pack: s.pack }); } catch (e) { add.textContent = String(e); }
      };
      wrap.append(add);
    }
    return wrap;
  }

  // ---- making a pack -----------------------------------------------------
  let draft = [];

  function openMaker() {
    draft = [];
    document.getElementById("maker-emoji").checked = false;
    document.getElementById("maker-title").value = "";
    document.getElementById("maker-error").textContent = "";
    drawDraft();
    document.getElementById("maker").showModal();
  }

  function drawDraft() {
    const list = document.getElementById("maker-items");
    list.replaceChildren();
    draft.forEach((d, i) => {
      const row = document.createElement("div");
      row.className = "draft";
      const emoji = document.createElement("input");
      emoji.value = d.emoji;
      emoji.placeholder = emojiPack() ? "name" : "😀";
      emoji.maxLength = emojiPack() ? 32 : 8;
      emoji.title = emojiPack()
        ? "The name typed as :name: (2 to 32 of a-z, 0-9 and _)"
        : "The emoji this sticker answers to";
      emoji.oninput = () => { d.emoji = emoji.value; };
      const drop = document.createElement("button");
      drop.type = "button";
      drop.className = "secondary";
      drop.textContent = "✕";
      drop.onclick = () => { draft.splice(i, 1); drawDraft(); };
      row.append(render(d, 72), emoji, drop);
      list.append(row);
    });
    // maya_chat::frame::MAX_PACK_ITEMS and maya_chat::emoji::MAX_EMOJI_PACK_ITEMS.
    document.getElementById("maker-count").textContent = `${draft.length} / ${emojiPack() ? 24 : 120}`;
  }

  const emojiPack = () => document.getElementById("maker-emoji").checked;

  async function addFiles(files) {
    const error = document.getElementById("maker-error");
    error.textContent = "";
    for (const f of files) {
      try {
        const item = await invoke("import_sticker", new Uint8Array(await f.arrayBuffer()));
        const name = emojiPack() ? `emoji${draft.length + 1}` : "🙂";
        if (!draft.some((d) => d.id === item.id)) draft.push({ ...item, emoji: name });
      } catch (e) {
        error.textContent = `${f.name}: ${e}`;
      }
    }
    drawDraft();
  }

  async function savePack() {
    const error = document.getElementById("maker-error");
    try {
      await invoke("create_pack", {
        title: document.getElementById("maker-title").value,
        items: draft.map((d) => ({ id: d.id, emoji: d.emoji })),
        emoji: emojiPack(),
      });
      document.getElementById("maker").close();
      await refresh();
    } catch (e) {
      error.textContent = String(e);
    }
  }

  function alertLine(e) {
    document.getElementById("chat-error").textContent = String(e);
  }

  function init(sender) {
    onSend = sender;
    const files = document.getElementById("maker-files");
    files.accept = ACCEPT;
    files.onchange = () => { addFiles([...files.files]); files.value = ""; };
    document.getElementById("maker-save").onclick = savePack;
    document.getElementById("maker-emoji").onchange = drawDraft;
    document.getElementById("maker-cancel").onclick = () => document.getElementById("maker").close();
    document.getElementById("sticker-toggle").onclick = () => {
      const panel = document.getElementById("sticker-panel");
      panel.hidden = !panel.hidden;
      if (!panel.hidden) refresh().catch(alertLine);
    };
    return refresh();
  }

  return { init, render, line, suggest, refresh, glyph, inline, customEmoji };
})();
