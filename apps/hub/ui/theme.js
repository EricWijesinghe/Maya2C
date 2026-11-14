// System / Light / Dark. Loaded in <head>, before the stylesheet paints, so a
// stored choice never flashes the other theme. "system" removes the attribute
// and tokens.css follows the OS, dark when the OS states no preference.
"use strict";

const Theme = (() => {
  const KEY = "hub.theme";
  const CHOICES = ["system", "light", "dark"];
  const listeners = [];
  const os = window.matchMedia("(prefers-color-scheme: light)");

  function stored() {
    try {
      const v = localStorage.getItem(KEY);
      return CHOICES.includes(v) ? v : "system";
    } catch {
      return "system"; // blocked storage: follow the OS
    }
  }

  let choice = stored();

  // The theme actually drawn, for canvases and the control's state.
  const resolved = () => (choice === "system" ? (os.matches ? "light" : "dark") : choice);

  function apply() {
    const root = document.documentElement;
    if (choice === "system") delete root.dataset.theme;
    else root.dataset.theme = choice;
    listeners.forEach((fn) => fn(resolved()));
  }

  function set(next) {
    if (!CHOICES.includes(next)) return false;
    choice = next;
    try {
      localStorage.setItem(KEY, next);
    } catch {
      // The choice still holds for this session.
    }
    apply();
    return true;
  }

  os.addEventListener("change", () => {
    if (choice === "system") apply();
  });
  apply();

  return { CHOICES, get: () => choice, resolved, set, onChange: (fn) => listeners.push(fn) };
})();
