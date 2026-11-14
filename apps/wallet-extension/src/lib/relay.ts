/**
 * The content script's relay, over an injectable window and sender so the
 * provider round trip can be tested without a browser.
 */
export type SendToBackground = (msg: { method: string; params: unknown }) => Promise<unknown>;

export function installRelay(win: Window, send: SendToBackground): void {
  win.addEventListener("message", (event: MessageEvent) => {
    // Only this window's own page, posting to us: not another frame, not a
    // message whose source is some other window.
    if (event.source !== win || event.origin !== win.location.origin) return;
    const data = event.data as { target?: unknown; id?: unknown; method?: unknown; params?: unknown } | null;
    if (!data || data.target !== "maya2c-content" || typeof data.id !== "number" || typeof data.method !== "string") return;
    const id = data.id;
    // `async` so a synchronous throw (an invalidated extension context) is a
    // rejection too, and a missing reply is an answer: a dapp never hangs.
    (async () => send({ method: data.method as string, params: data.params ?? null }))()
      .then((reply) => reply ?? { ok: false, code: 4900, message: "the wallet did not answer" })
      .catch((e: unknown) => ({ ok: false, code: 4900, message: e instanceof Error ? e.message : String(e) }))
      .then((reply) => {
        try {
          win.postMessage({ target: "maya2c-inpage", id, reply }, win.location.origin);
        } catch {
          // The page went away or has an opaque origin: nobody is left to answer.
        }
      });
  });
}
