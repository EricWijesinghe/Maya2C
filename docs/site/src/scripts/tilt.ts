// 3D tilt for [data-tilt] cards: the card leans toward the pointer, a few
// degrees at most. Mouse and pen only (a finger scrolls), never under reduced
// motion. One delegated listener for the whole page.
const MAX_DEG = 6;

export function enableTilt(): void {
  if (matchMedia("(prefers-reduced-motion: reduce)").matches || !matchMedia("(hover: hover)").matches) return;
  let current: HTMLElement | null = null;
  document.addEventListener("pointermove", (e) => {
    if (e.pointerType === "touch") return;
    const el = (e.target as Element | null)?.closest<HTMLElement>("[data-tilt]") ?? null;
    if (current && current !== el) { current.style.removeProperty("--tx"); current.style.removeProperty("--ty"); }
    current = el;
    if (!el) return;
    const r = el.getBoundingClientRect();
    el.style.setProperty("--tx", `${(((e.clientY - r.top) / r.height) - 0.5) * -2 * MAX_DEG}deg`);
    el.style.setProperty("--ty", `${(((e.clientX - r.left) / r.width) - 0.5) * 2 * MAX_DEG}deg`);
  }, { passive: true });
}
