/** Runs `start` once `el` comes within 200 px of the viewport, never before: widgets most visits never reach cost nothing. */
export function whenNear(el: Element, start: () => void): void {
  const io = new IntersectionObserver((es) => {
    if (es.some((e) => e.isIntersecting)) { io.disconnect(); start(); }
  }, { rootMargin: "200px 0px" });
  io.observe(el);
}
