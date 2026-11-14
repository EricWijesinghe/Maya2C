/** The page-side half of the provider: posts requests, matches replies by id. */

export class Maya2cProviderError extends Error {
  constructor(readonly code: number, message: string) {
    super(message);
    this.name = "Maya2cProviderError";
  }
}

export interface Maya2cProvider {
  readonly isMaya2c: true;
  request(args: { method: string; params?: unknown }): Promise<unknown>;
}

type Reply = { ok: true; result: unknown } | { ok: false; code: number; message: string };

/** Builds the provider over a window (a real one, or a test double). */
export function createProvider(win: Window): Maya2cProvider {
  let nextId = 1;
  const waiting = new Map<number, (reply: Reply) => void>();

  win.addEventListener("message", (event: MessageEvent) => {
    if (event.source !== win) return;
    const data = event.data as { target?: unknown; id?: unknown; reply?: Reply } | null;
    if (!data || data.target !== "maya2c-inpage" || typeof data.id !== "number" || !data.reply) return;
    const done = waiting.get(data.id);
    if (done) {
      waiting.delete(data.id);
      done(data.reply);
    }
  });

  return Object.freeze({
    isMaya2c: true as const,
    request({ method, params }: { method: string; params?: unknown }): Promise<unknown> {
      if (typeof method !== "string") return Promise.reject(new Maya2cProviderError(-32602, "method must be a string"));
      const id = nextId++;
      return new Promise((resolve, reject) => {
        waiting.set(id, (reply) =>
          reply.ok ? resolve(reply.result) : reject(new Maya2cProviderError(reply.code, reply.message)),
        );
        try {
          win.postMessage({ target: "maya2c-content", id, method, params: params ?? null }, win.location.origin);
        } catch (e) {
          waiting.delete(id);
          reject(new Maya2cProviderError(-32602, e instanceof Error ? e.message : "params could not be sent"));
        }
      });
    },
  });
}
