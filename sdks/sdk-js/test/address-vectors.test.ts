// Cross-language check: the same vectors the Rust crate (crates/address) reads.
// A change to the text form fails here and in Rust and Python together.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { AddressError, type AddressNetwork, decodeAddress, decodeAnyAddress, encodeAddress } from "../src/address.js";

interface ValidVector {
  name: string;
  network: AddressNetwork;
  id_hex: string;
  text: string;
}

interface RefusedVector {
  name: string;
  text: string;
  expect_network: AddressNetwork | null;
  error: string;
}

const vectors = JSON.parse(
  readFileSync(new URL("../../../spec/address-vectors.json", import.meta.url), "utf8"),
) as { valid: ValidVector[]; refused: RefusedVector[] };

const fromHex = (hex: string): Uint8Array => Uint8Array.from(Buffer.from(hex, "hex"));

describe("address text vectors (ADR-081)", () => {
  it("has the full vector set", () => {
    expect(vectors.valid.length).toBeGreaterThanOrEqual(13);
    expect(vectors.refused.length).toBeGreaterThanOrEqual(8);
  });

  it.each(vectors.valid)("encodes and decodes $name", ({ network, id_hex, text }) => {
    const id = fromHex(id_hex);
    expect(encodeAddress(network, id)).toBe(text.toLowerCase());
    expect(decodeAddress(network, text)).toEqual(id);
    expect(decodeAnyAddress(text)).toEqual({ network, id });
  });

  it.each(vectors.refused)("refuses $name with $error", ({ text, expect_network, error }) => {
    const attempt = () =>
      expect_network === null ? decodeAnyAddress(text) : decodeAddress(expect_network, text);
    let caught: unknown;
    try {
      attempt();
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(AddressError);
    expect((caught as AddressError).kind).toBe(error);
  });

  it("rejects an account id of the wrong length", () => {
    expect(() => encodeAddress("testnet", new Uint8Array(31))).toThrow(RangeError);
  });
});
