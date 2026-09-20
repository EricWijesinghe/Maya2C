/**
 * maya2c.js — TypeScript client for the Maya2C API gateway.
 *
 * # What this talks to
 *
 * The gateway (`maya-api-gateway`), not a node directly. A node's JSON-RPC port
 * has no authentication and exposes the miner interface; the gateway is the
 * layer that makes a public client possible. Pointing this at a node would work
 * for reads and would be the wrong thing to ship.
 *
 * # Signing lives in wasm, not here
 *
 * A Maya2C signature is a hybrid pair — ML-DSA-65 (FIPS 204) plus
 * SLH-DSA-SHA2-128s (FIPS 205), 11,165 bytes, both required. None of that is
 * reimplemented in TypeScript. `@maya2c/wasm` (built from `sdk-wasm`) carries
 * it, and `tests/hybrid_parity_tests.rs` in the Rust workspace pins that
 * implementation against the node's byte for byte.
 *
 * A JavaScript reimplementation of a post-quantum signature scheme would be a
 * second consensus-critical encoding maintained by hand. There is one already —
 * see the parity test — and one is the most this project should have.
 */

/** Balance and nonce for an account. */
export interface Balance {
  /** Spendable amount in base units. */
  balance: number;
  /** Next valid nonce. */
  nonce: number;
}

/** Circulating and total supply. */
export interface Supply {
  /** Units in circulation. */
  circulating: number;
  /** Units issued in total. */
  total: number;
}

/** A transaction the gateway accepted. */
export interface TransactionAccepted {
  /** Hex-encoded transaction hash. */
  hash: string;
}

/** A sealed submission the gateway accepted. */
export interface SealedAccepted {
  /** Correlation id returned by the gateway. */
  id: string;
  /** Ciphertext length in bytes, echoed back. */
  bytes: number;
}

/**
 * An error carrying the gateway's HTTP status.
 *
 * The status is kept because it is the only thing that distinguishes "you sent
 * something wrong" (4xx, do not retry) from "the node is unreachable" (502,
 * retry may help). A client that collapsed both into one string would retry
 * malformed requests forever.
 */
export class GatewayError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "GatewayError";
  }

  /** Whether retrying could plausibly succeed. */
  get retryable(): boolean {
    return this.status >= 500;
  }
}

/** Options for a {@link MayaClient}. */
export interface ClientOptions {
  /** Milliseconds before a request is abandoned. Defaults to 10 000. */
  timeoutMs?: number;
  /** Injected for tests; defaults to the global `fetch`. */
  fetch?: typeof globalThis.fetch;
}

/**
 * A client for one Maya2C gateway.
 *
 * Every method is a thin call plus error mapping. There is deliberately no
 * caching layer: a balance that is one block stale is a balance that builds an
 * invalid transaction, and a cache that a caller cannot see is worse than a
 * round trip they can.
 */
export class MayaClient {
  readonly #baseUrl: string;
  readonly #timeoutMs: number;
  readonly #fetch: typeof globalThis.fetch;

  constructor(baseUrl: string, options: ClientOptions = {}) {
    // Trailing slashes are stripped so `${base}/v1/...` never produces a
    // double slash, which some proxies treat as a different path.
    this.#baseUrl = baseUrl.replace(/\/+$/, "");
    this.#timeoutMs = options.timeoutMs ?? 10_000;
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
  }

  async #request<T>(path: string, init?: RequestInit): Promise<T> {
    // An AbortController per request: without a timeout a wedged gateway
    // leaves the caller hanging, which in a browser means a spinner that never
    // stops and no way for the user to tell what happened.
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.#timeoutMs);

    try {
      const response = await this.#fetch(`${this.#baseUrl}${path}`, {
        ...init,
        signal: controller.signal,
      });

      const text = await response.text();
      const body: unknown = text ? JSON.parse(text) : null;

      if (!response.ok) {
        const message =
          body && typeof body === "object" && "error" in body
            ? String((body as { error: unknown }).error)
            : `request failed with status ${response.status}`;
        throw new GatewayError(message, response.status);
      }

      return body as T;
    } catch (error) {
      if (error instanceof GatewayError) throw error;
      if (error instanceof Error && error.name === "AbortError") {
        throw new GatewayError(`request timed out after ${this.#timeoutMs}ms`, 504);
      }
      throw new GatewayError(
        error instanceof Error ? error.message : String(error),
        0,
      );
    } finally {
      clearTimeout(timer);
    }
  }

  /** Gateway liveness. Does not touch the node. */
  async health(): Promise<{ status: string }> {
    return this.#request("/health");
  }

  /** Balance and nonce for a hex address, with or without a `0x` prefix. */
  async getBalance(address: string): Promise<Balance> {
    return this.#request(`/v1/accounts/${encodeURIComponent(address)}`);
  }

  /** A block by height. */
  async getBlock(height: number): Promise<unknown> {
    if (!Number.isInteger(height) || height < 0) {
      throw new GatewayError("height must be a non-negative integer", 400);
    }
    return this.#request(`/v1/blocks/${height}`);
  }

  /** Circulating and total supply. */
  async getSupply(): Promise<Supply> {
    return this.#request("/v1/supply");
  }

  /** Submits a hex-encoded signed transaction. */
  async sendRawTransaction(raw: string): Promise<TransactionAccepted> {
    return this.#request("/v1/transactions", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ raw }),
    });
  }

  /**
   * Submits a threshold-encrypted transaction.
   *
   * The ciphertext is opaque to this client and to the gateway. Encrypting it
   * is the caller's job; neither layer holds a committee share, and a client
   * that could decrypt would defeat the sealed mempool it is submitting to.
   */
  async sendSealedTransaction(ciphertext: string): Promise<SealedAccepted> {
    return this.#request("/v1/sealed", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ciphertext }),
    });
  }

  /** Runs a GraphQL query against the gateway. */
  async graphql<T>(query: string, variables?: Record<string, unknown>): Promise<T> {
    const body = await this.#request<{ data?: T; errors?: unknown[] }>("/graphql", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ query, variables }),
    });

    // GraphQL returns 200 with an `errors` array rather than an HTTP error, so
    // a client that only checked the status would silently treat a rejected
    // query — including one refused by the complexity limit — as a success
    // with `data: null`.
    if (body.errors && body.errors.length > 0) {
      throw new GatewayError(
        `graphql: ${JSON.stringify(body.errors)}`,
        200,
      );
    }
    if (body.data === undefined) {
      throw new GatewayError("graphql response carried no data", 200);
    }
    return body.data;
  }
}
