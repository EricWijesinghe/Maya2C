// Runs fn once the page has loaded and the browser is idle, so decorative
// WebGL never competes with first paint. The timeout bounds the wait on a busy
// page; browsers without requestIdleCallback (Safari) fall back to a short delay.
export function whenIdle(fn: () => void): void {
  const go = () => {
    if ("requestIdleCallback" in window) window.requestIdleCallback(() => fn(), { timeout: 2000 });
    else window.setTimeout(fn, 300);
  };
  if (document.readyState === "complete") go();
  else window.addEventListener("load", go, { once: true });
}
