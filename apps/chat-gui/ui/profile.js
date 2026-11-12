// Profile cards and looks in the window (ADR-140 build PR 4). Rust checks and
// signs everything; this file draws. Nothing here fetches from the network:
// every image is bytes from the encrypted cache, drawn as a blob or data URL
// (the CSP allows no other source).
const Profiles = (() => {
  const invoke = window.__TAURI__.core.invoke;
  const $ = (id) => document.getElementById(id);
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
  let themes = [];
  let sharing = { cards: {}, default: null, per_contact: {} };
  let peer = null;
  let onError = () => {};

  async function bytesOf(id) {
    return new Uint8Array(await invoke("sticker_file", { id }));
  }

  // Imports a file through the sticker path (caps, encrypted cache) and
  // returns its id; Rust checks its role when the card or look is saved.
  async function importFile(input) {
    const file = input.files[0];
    if (!file) return null;
    const item = await invoke("import_sticker", new Uint8Array(await file.arrayBuffer()));
    return item.id;
  }

  // ---- looks ---------------------------------------------------------------
  // A still copy of an image, so an APNG or animated WebP wallpaper never moves.
  async function stillUrl(id) {
    const url = URL.createObjectURL(new Blob([await bytesOf(id)]));
    try {
      const img = new Image();
      await new Promise((ok, fail) => { img.onload = ok; img.onerror = fail; img.src = url; });
      const c = document.createElement("canvas");
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      c.getContext("2d").drawImage(img, 0, 0);
      return c.toDataURL("image/png");
    } finally {
      URL.revokeObjectURL(url);
    }
  }

  async function applyLook(look) {
    const root = document.documentElement;
    const base = themes.find((t) => t.id === (look.theme || "forged")) || themes[0];
    // Every allowed token is reset first, so a previous chat's colours never linger.
    for (const [k] of themes[0]?.tokens || []) root.style.removeProperty(k);
    root.style.removeProperty("--line");
    for (const [k, v] of base ? base.tokens : []) root.style.setProperty(k, v);
    for (const [k, v] of Object.entries(look.tokens || {})) root.style.setProperty(k, v);
    const lines = $("lines");
    const w = look.wallpaper || { kind: "none" };
    lines.style.backgroundImage = "";
    if (w.kind === "gradient") {
      lines.style.backgroundImage = `linear-gradient(${w.angle}deg, ${w.from}, ${w.to})`;
    } else if (w.kind === "image") {
      try {
        lines.style.backgroundImage = `url(${await stillUrl(w.id)})`;
      } catch {
        lines.style.backgroundImage = "";
      }
    }
  }

  function lookFromForm(prefix) {
    const kind = $(`${prefix}-wall`).value;
    const wallpaper =
      kind === "gradient"
        ? { kind, from: $(`${prefix}-from`).value, to: $(`${prefix}-to`).value, angle: 135 }
        : kind === "image" && $(`${prefix}-wall-id`).value
          ? { kind, id: $(`${prefix}-wall-id`).value }
          : { kind: "none" };
    return { theme: $(`${prefix}-theme`).value, tokens: {}, wallpaper };
  }

  function fillThemes(select, withInherit) {
    select.replaceChildren();
    if (withInherit) select.append(new Option("As everywhere", ""));
    for (const t of themes) select.append(new Option(t.label, t.id));
  }

  async function saveLook(prefix, forPeer) {
    try {
      await invoke("set_look", { peer: forPeer, look: lookFromForm(prefix) });
      if (peer) await applyLook(await invoke("chat_look", { peer }));
    } catch (e) {
      onError(e);
    }
  }

  function wireLook(prefix, forPeer) {
    const wall = $(`${prefix}-wall`);
    const sync = () => {
      $(`${prefix}-grad`).hidden = wall.value !== "gradient";
      $(`${prefix}-file`).hidden = wall.value !== "image";
    };
    wall.addEventListener("sync", sync);
    wall.onchange = () => { sync(); saveLook(prefix, forPeer()); };
    $(`${prefix}-theme`).onchange = () => saveLook(prefix, forPeer());
    $(`${prefix}-from`).onchange = $(`${prefix}-to`).onchange = () => saveLook(prefix, forPeer());
    $(`${prefix}-file`).onchange = async (ev) => {
      try {
        $(`${prefix}-wall-id`).value = (await importFile(ev.target)) || "";
        await saveLook(prefix, forPeer());
      } catch (e) {
        onError(e);
      }
    };
    sync();
  }

  // ---- cards ---------------------------------------------------------------
  // The avatar, with the animated frame around it. The frame plays only
  // without the OS reduced-motion setting, and shows its first frame otherwise.
  function cardView(card) {
    const box = document.createElement("span");
    box.className = "card-avatar";
    if (card.avatar) {
      const img = document.createElement("img");
      img.alt = "";
      bytesOf(card.avatar.id)
        .then((b) => {
          // An SVG needs its type to draw; as an <img> no script in it runs.
          const head = new TextDecoder().decode(b.slice(0, 256)).replace(/^﻿/, "").trimStart();
          const type = head.startsWith("<") ? "image/svg+xml" : "";
          img.src = URL.createObjectURL(new Blob([b], { type }));
        })
        .catch(() => img.remove());
      box.append(img);
    }
    if (card.frame && window.lottie) {
      const ring = document.createElement("span");
      ring.className = "card-frame";
      ring.setAttribute("aria-hidden", "true");
      box.append(ring);
      bytesOf(card.frame.id)
        .then(async (b) => {
          if (b[0] === 0x1f) {
            const s = new Blob([b]).stream().pipeThrough(new DecompressionStream("gzip"));
            b = new Uint8Array(await new Response(s).arrayBuffer());
          }
          const anim = window.lottie.loadAnimation({
            container: ring, renderer: "svg", loop: true, autoplay: false,
            animationData: JSON.parse(new TextDecoder().decode(b)),
          });
          const sync = () => (reduced.matches ? anim.goToAndStop(0, true) : anim.play());
          reduced.addEventListener("change", sync);
          sync();
        })
        .catch(() => ring.remove());
    }
    return box;
  }

  async function drawPeerCard() {
    const slot = $("peer-card");
    slot.replaceChildren();
    if (!peer) return;
    const card = await invoke("peer_card", { peer });
    if (!card) return;
    const name = document.createElement("strong");
    name.textContent = card.name; // textContent: a name is never markup
    const bio = document.createElement("span");
    bio.className = "hint card-bio";
    bio.textContent = card.bio;
    slot.append(cardView(card), name, bio);
  }

  function drawShowSelect() {
    const sel = $("card-show");
    sel.replaceChildren(new Option("Default card", "default"));
    for (const key of Object.keys(sharing.cards)) sel.append(new Option(`Card “${key}”`, `card:${key}`));
    sel.append(new Option("No card", "nothing"));
    const choice = sharing.per_contact[peer];
    sel.value = !choice || choice === "default" ? "default" : choice === "nothing" ? "nothing" : `card:${choice.card}`;
  }

  function showValue(v) {
    return v === "default" ? "default" : v === "nothing" ? "nothing" : { card: v.slice(5) };
  }

  async function saveCard() {
    $("card-error").textContent = "";
    try {
      const draft = {
        name: $("card-name").value,
        bio: $("card-bio").value,
        avatar: $("card-avatar-id").value || null,
        frame: $("card-frame-id").value || null,
        theme: "",
      };
      sharing = await invoke("save_card", { key: $("card-key").value || "main", draft });
      $("card-error").textContent = "Saved on this computer. It is sent only when you press “Send my card” in a chat.";
      if (peer) drawShowSelect();
    } catch (e) {
      $("card-error").textContent = e;
    }
  }

  async function init(errorSink) {
    onError = errorSink;
    const got = await invoke("looks");
    themes = got.themes;
    sharing = await invoke("my_cards");
    fillThemes($("look-theme"), false);
    fillThemes($("chat-theme"), true);
    $("look-theme").value = got.looks.global.theme || "forged";
    $("look-wall").value = got.looks.global.wallpaper?.kind || "none";
    wireLook("look", () => null);
    wireLook("chat", () => peer);
    for (const [input, hidden] of [["card-avatar", "card-avatar-id"], ["card-frame", "card-frame-id"]]) {
      $(input).onchange = async (ev) => {
        try {
          $(hidden).value = (await importFile(ev.target)) || "";
        } catch (e) {
          $("card-error").textContent = e;
        }
      };
    }
    const main = sharing.cards.main;
    if (main) {
      $("card-name").value = main.name;
      $("card-bio").value = main.bio;
    }
    $("card-save").onclick = saveCard;
    $("card-show").onchange = async () => {
      try {
        sharing = await invoke("set_card_visibility", { peer, show: showValue($("card-show").value) });
      } catch (e) {
        onError(e);
      }
    };
    $("card-send").onclick = async () => {
      try {
        await invoke("send_card", { peer });
        onError("");
      } catch (e) {
        onError(e);
      }
    };
    await applyLook(got.looks.global);
  }

  async function open(addr) {
    peer = addr;
    $("card-bar").hidden = false;
    drawShowSelect();
    const look = await invoke("looks");
    const own = look.looks.per_chat[addr];
    $("chat-theme").value = own?.theme || "";
    $("chat-wall").value = own?.wallpaper?.kind || "none";
    $("chat-wall").dispatchEvent(new Event("sync"));
    await applyLook(await invoke("chat_look", { peer: addr }));
    await drawPeerCard();
  }

  return { init, open, refresh: drawPeerCard };
})();
