// The v4 payload codec against spec/tests/v4-client-vectors.json (ADR-135 I7).
// The vectors are written by the node, so a byte or a word this SDK gets
// differently fails here, in Python and in Rust together.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { PayloadError, decodePayload, describePayload, encodePayload, type Fields } from "../src/v4.js";

interface Vector {
  label: string;
  fields: Fields;
  payload: string;
  summary: string;
  careful: boolean;
  details: [string, string][];
}

const { vectors } = JSON.parse(
  readFileSync(new URL("../../../spec/tests/v4-client-vectors.json", import.meta.url), "utf8"),
) as { vectors: Vector[] };

const fromHex = (hex: string): Uint8Array => Uint8Array.from(Buffer.from(hex, "hex"));
const toHex = (bytes: Uint8Array): string => Buffer.from(bytes).toString("hex");

describe("v4 payload vectors", () => {
  it("covers every form", () => {
    expect(new Set(vectors.map((v) => v.label)).size).toBe(14);
  });

  it.each(vectors.map((v) => [v.label, v] as const))("%s encodes, decodes and reads as the node", (_, v) => {
    expect(toHex(encodePayload(v.label, v.fields))).toBe(v.payload);
    expect(decodePayload(fromHex(v.payload))).toEqual([v.label, v.fields]);
    const shown = describePayload(v.label, v.fields);
    expect(shown.summary).toBe(v.summary);
    expect(shown.careful).toBe(v.careful);
    expect(shown.details).toEqual(v.details);
  });

  it("refuses truncated, trailing and unknown payloads", () => {
    for (const v of vectors) {
      const raw = fromHex(v.payload);
      expect(() => decodePayload(raw.subarray(0, raw.length - 1))).toThrow(PayloadError);
      expect(() => decodePayload(Uint8Array.from([...raw, 0]))).toThrow(PayloadError);
    }
    for (const tag of [0, 54, 69, 255]) {
      expect(() => decodePayload(Uint8Array.from([tag, ...new Array<number>(64).fill(0)]))).toThrow(PayloadError);
    }
  });

  it("refuses fields that do not fit instead of truncating them", () => {
    expect(() => encodePayload("renew_name", { label: "Alice", periods: "1" })).toThrow(PayloadError);
    expect(() => encodePayload("renew_name", { label: "alice", periods: "256" })).toThrow(PayloadError);
    expect(() => encodePayload("burn_asset", { asset: "00".repeat(31), amount: "1" })).toThrow(PayloadError);
    expect(() => encodePayload("burn_asset", { asset: "00".repeat(32), amount: (1n << 256n).toString() })).toThrow(PayloadError);
    expect(() => encodePayload("update_name", { label: "alice", op: "set_stealth" })).toThrow(PayloadError);
  });
});
