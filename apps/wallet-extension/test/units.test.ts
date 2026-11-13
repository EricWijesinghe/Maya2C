import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { formatCoins, fromHex, parseBaseUnits, parseCoins, toHex, toLe32, COIN_DECIMALS } from "../src/lib/bytes.js";
import { addressNetwork, classify, fingerprint, parseRecipient, MAINNET_GENESIS, TESTNET_GENESIS } from "../src/lib/network.js";
import { accountsFor, grant, grantableOrigin, isConnected, revoke } from "../src/lib/permissions.js";
import { FEE_COLLECTOR_HEX, routeOf } from "../src/lib/service.js";
import { encodeAddress } from "../../../sdks/sdk-js/src/address.js";

describe("amounts", () => {
  it("uses the decimals spec/tests/units.json pins", () => {
    const units = JSON.parse(readFileSync(new URL("../../../spec/tests/units.json", import.meta.url), "utf8"));
    expect(COIN_DECIMALS).toBe(units.coin_decimals);
    expect(parseCoins("1").toString()).toBe(units.base_units_per_coin);
  });

  it("parses and formats coins exactly, refusing more than 24 decimals", () => {
    expect(parseCoins("1.5")).toBe(15n * 10n ** 23n);
    expect(formatCoins(parseCoins("0.000000000000000000000001"))).toBe("0.000000000000000000000001");
    expect(() => parseCoins("1.0000000000000000000000001")).toThrow();
    expect(() => parseCoins("-1")).toThrow();
    expect(() => parseCoins("1e3")).toThrow();
  });

  it("encodes 256-bit little-endian and refuses overflow", () => {
    expect(toHex(toLe32(0x0102n))).toBe("0201" + "00".repeat(30));
    expect(() => toLe32(1n << 256n)).toThrow();
    expect(() => parseBaseUnits((1n << 256n).toString())).toThrow();
    expect(() => parseBaseUnits("12.5")).toThrow();
  });

  it("round-trips hex and refuses non-hex", () => {
    expect(toHex(fromHex("0x00ff10"))).toBe("00ff10");
    expect(() => fromHex("abc")).toThrow();
    expect(() => fromHex("zz")).toThrow();
  });
});

describe("network and recipient checks (the #446 rule)", () => {
  const id = new Uint8Array(32).fill(7);
  const genesis = "ab".repeat(32);

  it("classifies by genesis, and an unpinned mainnet name as an impostor", () => {
    expect(classify("maya-testnet-1", genesis)).toBe("test");
    expect(classify("maya-mainnet", genesis)).toBe("impostor");
    expect(fingerprint(genesis)).toBe("abababab…abababab");
  });

  it("refuses a mainnet address on a test chain and pays a testnet one", () => {
    expect(() => parseRecipient(encodeAddress("mainnet", id), "test")).toThrow(/mainnet/);
    expect(parseRecipient(encodeAddress("testnet", id), "test").named).toBe("testnet");
  });

  it("refuses a testnet invoice on mainnet", () => {
    expect(() => parseRecipient(encodeAddress("testnet", id), "mainnet")).toThrow(/testnet/);
    expect(parseRecipient(encodeAddress("mainnet", id), "mainnet").named).toBe("mainnet");
  });

  it("accepts a bare hex id but records that it names no network", () => {
    expect(parseRecipient("07".repeat(32), "test")).toEqual({ id, named: null });
  });

  it("refuses everything on an impostor chain", () => {
    expect(() => parseRecipient("07".repeat(32), "impostor")).toThrow(/impostor|unpinned/);
  });

  it("pins the same genesis lists as sdk-wasm, and writes an unpinned chain's addresses as dmaya", () => {
    // sdk-wasm's lists are pinned to the desktop's by desktop_parity_tests.rs;
    // this pins the TypeScript copy to sdk-wasm's.
    const rust = readFileSync(new URL("../../../sdks/sdk-wasm/src/wallet.rs", import.meta.url), "utf8");
    const list = (name: string) => {
      const m = rust.match(new RegExp(`pub const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\];`));
      if (!m) throw new Error(`${name} not found in wallet.rs`);
      return [...m[1].matchAll(/"([0-9a-fA-F]+)"/g)].map((x) => x[1].toLowerCase());
    };
    expect([...MAINNET_GENESIS].map((g) => g.toLowerCase())).toEqual(list("MAINNET_GENESIS"));
    expect([...TESTNET_GENESIS].map((g) => g.toLowerCase())).toEqual(list("TESTNET_GENESIS"));
    expect(addressNetwork(genesis)).toBe("devnet");
  });

  it("knows the node's fee collector", () => {
    // Bytes of FEE_COLLECTOR in crates/node/src/state/fees.rs.
    expect(FEE_COLLECTOR_HEX).toBe("6d61796132632f6665652f636f6c6c6563746f722f7631000000000000000000");
  });
});

describe("per-origin permissions", () => {
  it("grants only https and loopback http origins", () => {
    expect(grantableOrigin("https://dapp.example/path?q=1")).toBe("https://dapp.example");
    expect(grantableOrigin("http://localhost:5173/")).toBe("http://localhost:5173");
    expect(grantableOrigin("http://dapp.example/")).toBeNull();
    expect(grantableOrigin("file:///C:/x.html")).toBeNull();
    expect(grantableOrigin("data:text/html,hi")).toBeNull();
    expect(grantableOrigin(undefined)).toBeNull();
  });

  it("keys grants by exact origin, immutably", () => {
    const empty = {};
    const one = grant(empty, "https://a.example", [0]);
    expect(empty).toEqual({});
    expect(isConnected(one, "https://a.example")).toBe(true);
    expect(isConnected(one, "https://a.example:8443")).toBe(false);
    expect(isConnected(one, "https://sub.a.example")).toBe(false);
    expect(accountsFor(one, "__proto__")).toEqual([]);
    expect(isConnected(revoke(one, "https://a.example"), "https://a.example")).toBe(false);
    expect(() => grant(empty, "https://a.example", [])).toThrow();
  });

  it("routes by the browser's sender record, not by the message", () => {
    const ext = "abcdef";
    const base = "chrome-extension://abcdef/";
    expect(routeOf({ id: ext, url: `${base}popup.html` }, ext, base)).toEqual({ kind: "popup" });
    expect(routeOf({ id: ext, url: "https://d.example/x", origin: "https://d.example", tab: {}, frameId: 0 }, ext, base))
      .toEqual({ kind: "provider", origin: "https://d.example" });
    // An iframe does not borrow the top page's grant.
    expect(routeOf({ id: ext, url: "https://d.example/x", tab: {}, frameId: 3 }, ext, base)).toEqual({ kind: "refuse" });
    // Another extension, or a non-grantable page, is refused.
    expect(routeOf({ id: "other", url: `${base}popup.html` }, ext, base)).toEqual({ kind: "refuse" });
    expect(routeOf({ id: ext, url: "http://d.example/", tab: {}, frameId: 0 }, ext, base)).toEqual({ kind: "refuse" });
    // A web page cannot pose as the popup: its url is never under the extension base.
    expect(routeOf({ id: ext, url: "https://evil.example/chrome-extension://abcdef/", tab: {}, frameId: 0 }, ext, base))
      .toEqual({ kind: "provider", origin: "https://evil.example" });
  });
});
