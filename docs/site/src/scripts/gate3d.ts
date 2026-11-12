// When the site may run a WebGL scene, and when it starts one. Every 3D
// widget has a static fallback in its HTML; this decides whether it is replaced.
// No 3D under reduced motion, on save-data, on low-end devices, or without WebGL.
const LOW_CORES = 4;
const LOW_MEMORY_GB = 4;

export function canRun3D(): boolean {
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return false;
  const nav = navigator as Navigator & { deviceMemory?: number; connection?: { saveData?: boolean } };
  if (nav.connection?.saveData) return false;
  if ((nav.hardwareConcurrency ?? LOW_CORES + 1) <= LOW_CORES) return false;
  if (nav.deviceMemory !== undefined && nav.deviceMemory < LOW_MEMORY_GB) return false;
  try {
    return !!document.createElement("canvas").getContext("webgl2");
  } catch {
    return false;
  }
}

/** Runs `start` once `el` comes within a screen of the viewport, never before. */
export function whenNear(el: Element, start: () => void): void {
  const io = new IntersectionObserver((es) => {
    if (es.some((e) => e.isIntersecting)) { io.disconnect(); start(); }
  }, { rootMargin: "200px 0px" });
  io.observe(el);
}

/** Calls `set(running)` as the element enters or leaves the screen and as the tab hides or shows. */
export function onRunnable(el: Element, set: (running: boolean) => void): void {
  let onScreen = true;
  const apply = () => set(onScreen && !document.hidden);
  new IntersectionObserver(([e]) => { onScreen = e.isIntersecting; apply(); }).observe(el);
  document.addEventListener("visibilitychange", apply);
}

/** Device pixel ratio, capped at 2: sharper than that costs fill rate nobody can see. */
export const dpr = () => Math.min(window.devicePixelRatio || 1, 2);
