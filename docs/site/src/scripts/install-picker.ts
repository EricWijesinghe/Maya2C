// Behaviour for components/InstallPicker.astro. The walkthroughs are already
// in the page; this only chooses which one shows, copies commands, counts
// finished steps, and keeps the page's OS tabs (<Tabs syncKey="os">) on the
// same computer as the picker.

type State = { os: string; v: Record<string, string>; arch: string; m: string };

const STORE_KEY = "maya2c-install-picker";
const TABS_SELECTOR = 'starlight-tabs[data-sync-key="os"]';
const COPIED_MS = 1600;

// Storage can be missing or throw (private windows, blocked site data): the
// picker then simply starts from its defaults.
function load(): Partial<State> {
  try {
    return JSON.parse(localStorage.getItem(STORE_KEY) ?? "{}") as Partial<State>;
  } catch {
    return {};
  }
}
function save(s: State): void {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(s));
  } catch {
    /* not remembering is fine */
  }
}

function initialState(root: HTMLElement): State {
  const [os, v, arch, m] = (root.dataset.default ?? "windows.11.-.oneline").split(".");
  const stored = load();
  const state: State = {
    os: stored.os ?? os,
    v: { windows: v, mac: "apple", linux: "debian", ...(stored.v ?? {}) },
    arch: stored.arch ?? (arch === "-" ? "x86_64" : arch),
    m: stored.m ?? m,
  };
  // A shared link wins over what this browser remembered.
  const q = new URLSearchParams(location.search);
  if (q.get("os")) state.os = q.get("os")!;
  if (q.get("v")) state.v[state.os] = q.get("v")!;
  if (q.get("arch")) state.arch = q.get("arch")!;
  if (q.get("m")) state.m = q.get("m")!;
  return state;
}

function key(s: State): string {
  return [s.os, s.v[s.os], s.os === "linux" ? s.arch : "-", s.m].join(".");
}

function writeUrl(s: State): void {
  const q = new URLSearchParams(location.search);
  q.set("os", s.os);
  q.set("v", s.v[s.os]);
  if (s.os === "linux") q.set("arch", s.arch);
  else q.delete("arch");
  q.set("m", s.m);
  history.replaceState(null, "", `${location.pathname}?${q}${location.hash}`);
}

function markRow(row: HTMLElement, value: string): void {
  row.querySelectorAll<HTMLButtonElement>(".ip-opt").forEach((b) => {
    b.setAttribute("aria-checked", String(b.dataset.value === value));
    b.tabIndex = b.dataset.value === value ? 0 : -1;
  });
}

function syncTabs(label: string | undefined): void {
  if (!label) return;
  const tabs = document.querySelector(TABS_SELECTOR);
  const tab = [...(tabs?.querySelectorAll<HTMLElement>('[role="tab"]') ?? [])].find(
    (t) => t.textContent?.trim() === label,
  );
  if (tab && tab.getAttribute("aria-selected") !== "true") tab.click();
}

function updateProgress(root: HTMLElement, walk: HTMLElement | null): void {
  const steps = [...(walk?.querySelectorAll<HTMLElement>(".ip-step") ?? [])];
  const done = steps.filter((s) => s.classList.contains("is-done")).length;
  const fill = root.querySelector<HTMLElement>(".ip-meter-fill");
  const count = root.querySelector<HTMLElement>(".ip-count");
  if (fill) fill.style.width = steps.length ? `${(100 * done) / steps.length}%` : "0";
  if (count) count.textContent = steps.length ? `${done} of ${steps.length} steps done` : "No steps for this choice yet";
}

function showStep(walk: HTMLElement | null, index: number): void {
  walk?.querySelectorAll<HTMLElement>(".ip-step").forEach((s, i) => s.classList.toggle("is-current", i === index));
}

async function copy(btn: HTMLButtonElement): Promise<void> {
  const text = btn.parentElement?.querySelector("code")?.textContent ?? "";
  try {
    await navigator.clipboard.writeText(text);
    btn.textContent = "Copied";
  } catch {
    // No clipboard permission: select the text so Ctrl+C / ⌘C works.
    const code = btn.parentElement?.querySelector("code");
    if (code) getSelection()?.selectAllChildren(code);
    btn.textContent = "Press Ctrl+C";
  }
  btn.classList.add("is-copied");
  setTimeout(() => {
    btn.textContent = "Copy";
    btn.classList.remove("is-copied");
  }, COPIED_MS);
}

export function initInstallPicker(root: HTMLElement): void {
  const state = initialState(root);
  const rows = [...root.querySelectorAll<HTMLElement>(".ip-row")];

  const render = (fromTabs = false): void => {
    rows.forEach((row) => {
      const group = row.dataset.group;
      row.hidden = Boolean(group) && group !== state.os;
      const name = row.dataset.row;
      if (name === "os") markRow(row, state.os);
      else if (name === "v" && group) markRow(row, state.v[group]);
      else if (name === "arch") markRow(row, state.arch);
      else if (name === "m") markRow(row, state.m);
    });
    const want = key(state);
    let current: HTMLElement | null = null;
    root.querySelectorAll<HTMLElement>(".ip-walk").forEach((w) => {
      w.hidden = w.dataset.combo !== want;
      if (!w.hidden) current = w;
    });
    showStep(current, 0);
    updateProgress(root, current);
    save(state);
    writeUrl(state);
    if (!fromTabs) syncTabs((current as HTMLElement | null)?.dataset.tab);
  };

  root.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const opt = target.closest<HTMLButtonElement>(".ip-opt");
    if (opt) {
      const row = opt.closest<HTMLElement>(".ip-row")!;
      const value = opt.dataset.value!;
      if (row.dataset.row === "os") state.os = value;
      else if (row.dataset.row === "v") state.v[row.dataset.group!] = value;
      else if (row.dataset.row === "arch") state.arch = value;
      else state.m = value;
      render();
      return;
    }
    const copyBtn = target.closest<HTMLButtonElement>(".ip-copy");
    if (copyBtn) {
      void copy(copyBtn);
      return;
    }
    const nav = target.closest<HTMLButtonElement>(".ip-next, .ip-back");
    if (nav) {
      const step = nav.closest<HTMLElement>(".ip-step")!;
      const index = Number(step.dataset.step) + (nav.classList.contains("ip-next") ? 1 : -1);
      if (nav.classList.contains("ip-next")) {
        step.classList.add("is-done");
        const box = step.querySelector<HTMLInputElement>(".ip-done-box");
        if (box) box.checked = true;
      }
      const walk = step.closest<HTMLElement>(".ip-walk");
      showStep(walk, index);
      updateProgress(root, walk);
    }
  });

  root.addEventListener("change", (e) => {
    const target = e.target as HTMLInputElement;
    if (target.classList.contains("ip-done-box")) {
      const step = target.closest<HTMLElement>(".ip-step")!;
      step.classList.toggle("is-done", target.checked);
      updateProgress(root, step.closest<HTMLElement>(".ip-walk"));
    } else if (target.classList.contains("ip-guided-toggle")) {
      root.classList.toggle("is-guided", target.checked);
    }
  });

  // Arrow keys move within a row, as in any radio group.
  root.addEventListener("keydown", (e) => {
    const opt = (e.target as HTMLElement).closest<HTMLButtonElement>(".ip-opt");
    if (!opt || !["ArrowRight", "ArrowLeft", "ArrowDown", "ArrowUp"].includes(e.key)) return;
    const opts = [...opt.parentElement!.querySelectorAll<HTMLButtonElement>(".ip-opt")];
    const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : -1;
    const next = opts[(opts.indexOf(opt) + step + opts.length) % opts.length];
    e.preventDefault();
    next.focus();
    next.click();
  });

  // A reader who switches a tab further down moves the picker too.
  const tabLabels: Record<string, string> = { Windows: "windows", Mac: "mac", Linux: "linux" };
  document.querySelectorAll(TABS_SELECTOR).forEach((tabs) =>
    tabs.addEventListener("click", (e) => {
      const tab = (e.target as HTMLElement).closest('[role="tab"]');
      const os = tab ? tabLabels[tab.textContent?.trim() ?? ""] : undefined;
      if (os && os !== state.os) {
        state.os = os;
        render(true);
      }
    }),
  );

  render();
}
