import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { wasm } from "./wasm.js";
import { openVault, sealVault } from "../src/lib/vault.js";
import { fromHex } from "../src/lib/bytes.js";
import { encodeAddress } from "../../../sdks/sdk-js/src/address.js";

const vectors = JSON.parse(
  readFileSync(new URL("../../../spec/wallet-derivation-vectors.json", import.meta.url), "utf8"),
).vectors as { mnemonic: string; passphrase: string; account: number; index: number; address: string }[];

describe("derivation matches the desktop wallet", () => {
  // The same file sdks/sdk-wasm/tests/desktop_parity_tests.rs checks against
  // maya-wallet-core, so desktop == Rust copy == this wasm build.
  it.each(vectors)("$account/$index of '$mnemonic' (passphrase '$passphrase')", (v) => {
    const seed = wasm.WalletSeed.fromMnemonic(v.mnemonic, v.passphrase);
    try {
      expect(seed.addressAt(v.account, v.index)).toBe(v.address);
    } finally {
      seed.free();
    }
  });

  it("refuses a mistyped word through the checksum", () => {
    expect(() => wasm.validateMnemonic(vectors[0].mnemonic.replace("about", "abandon"))).toThrow();
  });

  it("makes a 24-word phrase from 32 bytes of entropy", () => {
    const words = wasm.mnemonicFromEntropy(new Uint8Array(32).fill(0xff));
    expect(words.split(" ")).toHaveLength(24);
    expect(words).toBe(vectors[4].mnemonic);
  });
});

describe("hybrid transfer signing", () => {
  it("signs a transfer both halves of which verify, bound to the chain", () => {
    const seed = wasm.WalletSeed.fromMnemonic(vectors[0].mnemonic, "");
    const chain = new Uint8Array(32).fill(0x5a);
    const zero = new Uint8Array(32);
    const amount = new Uint8Array(32);
    amount[0] = 9;
    const to = new Uint8Array(32).fill(1);
    const shown = seed.confirmTransfer(0, 0, to, amount, zero, zero, 4n, chain, new Uint8Array());
    const hash = shown.signingHash;
    shown.free();
    const raw = seed.signTransfer(0, 0, to, amount, zero, zero, 4n, chain, hash);
    expect(raw.length).toBe(wasm.transferWireLen(false));
    expect(raw[0]).toBe(5); // plain-transfer wire version
    // Deterministic, as the node signs: the same inputs give the same bytes.
    expect(seed.signTransfer(0, 0, to, amount, zero, zero, 4n, chain, hash)).toEqual(raw);
    expect(() => seed.signTransfer(0, 0, new Uint8Array(31), amount, zero, zero, 4n, chain, hash)).toThrow(/32 bytes/);
    seed.free();
  });

  it("refuses to sign when the hash names another transfer or another chain", () => {
    const seed = wasm.WalletSeed.fromMnemonic(vectors[0].mnemonic, "");
    const chain = new Uint8Array(32).fill(0x5a);
    const zero = new Uint8Array(32);
    const to = new Uint8Array(32).fill(1);
    const amount = new Uint8Array(32);
    amount[0] = 9;
    const shown = seed.confirmTransfer(0, 0, to, amount, zero, zero, 4n, chain, new Uint8Array());
    const hash = shown.signingHash;
    shown.free();
    const other = new Uint8Array(32);
    other[0] = 10;
    expect(() => seed.signTransfer(0, 0, to, other, zero, zero, 4n, chain, hash)).toThrow(/nothing was signed/);
    expect(() => seed.signTransfer(0, 0, to, amount, zero, zero, 5n, chain, hash)).toThrow(/nothing was signed/);
    expect(() => seed.signTransfer(0, 0, to, amount, zero, zero, 4n, new Uint8Array(32).fill(0x5b), hash))
      .toThrow(/nothing was signed/);
    expect(() => seed.signTransfer(0, 0, to, amount, zero, zero, 4n, chain, "")).toThrow(/nothing was signed/);
    seed.free();
  });

  it("renders the confirm screen in the desktop's words, every address in full Bech32m", () => {
    const seed = wasm.WalletSeed.fromMnemonic(vectors[0].mnemonic, "");
    const amount = new Uint8Array(32);
    amount[0] = 95;
    const balance = new Uint8Array(32);
    balance[0] = 100;
    const zero = new Uint8Array(32);
    const shown = seed.confirmTransfer(0, 0, new Uint8Array(32).fill(1), amount, zero, zero, 4n,
      new Uint8Array(32).fill(0x5a), balance);
    // An unpinned genesis is a development chain, as on the desktop.
    expect(shown.addressPrefix).toBe("dmaya");
    expect(shown.recipient).toMatch(/^dmaya1( [0-9a-z]{1,4})+$/);
    expect(shown.sender.replace(/ /g, "")).toBe(encodeAddress("devnet", fromHex(vectors[0].address)));
    expect(shown.description).toContain("95 base units");
    expect(shown.warnings).toEqual(["This sends 90% or more of your balance."]);
    shown.free();
    seed.free();
  });
});

describe("vault", () => {
  it("round-trips under Argon2id and refuses a wrong password", async () => {
    const record = await sealVault(wasm.vaultKey, "secret words", "correct horse battery");
    expect(record.ct).not.toContain("secret");
    expect(await openVault(wasm.vaultKey, record, "correct horse battery")).toBe("secret words");
    await expect(openVault(wasm.vaultKey, record, "wrong horse battery")).rejects.toThrow("wrong password");
  });

  it("refuses a short password and a tampered ciphertext", async () => {
    await expect(sealVault(wasm.vaultKey, "x", "elevenchars")).rejects.toThrow(/12/);
    const record = await sealVault(wasm.vaultKey, "secret words", "correct horse battery");
    const ct = atob(record.ct);
    const flipped = btoa(String.fromCharCode(ct.charCodeAt(0) ^ 1) + ct.slice(1));
    await expect(openVault(wasm.vaultKey, { ...record, ct: flipped }, "correct horse battery")).rejects.toThrow("wrong password");
    // The salt is bound as associated data: swapping it fails the tag too.
    const other = await sealVault(wasm.vaultKey, "secret words", "correct horse battery");
    await expect(openVault(wasm.vaultKey, { ...record, salt: other.salt }, "correct horse battery")).rejects.toThrow("wrong password");
  });
});
