/**
 * The mnemonic at rest: AES-256-GCM (WebCrypto) under a key from Argon2id
 * (sdk-wasm, the desktop vault's parameters: 64 MiB, 3 passes, 1 lane).
 *
 * WebCrypto has no Argon2, and PBKDF2 is the weaker choice against GPU
 * guessing, so the KDF runs in wasm. The cipher stays in WebCrypto: AES-GCM
 * there is a platform primitive, not code this extension ships.
 */
import { wipe } from "./bytes.js";

/** Derives the 32-byte vault key. Injected so tests can run the real wasm. */
export type VaultKdf = (password: Uint8Array, salt: Uint8Array) => Uint8Array;

export interface VaultRecord {
  v: 1;
  kdf: "argon2id-m65536-t3-p1";
  salt: string;
  iv: string;
  ct: string;
}

/** Minimum password length: the vault file is readable by anyone with disk access. */
export const MIN_PASSWORD = 12;
const KDF_LABEL = "argon2id-m65536-t3-p1";

/** Binds the format and salt to the ciphertext, so neither can be swapped later. */
const aad = (salt: string): Uint8Array<ArrayBuffer> => new TextEncoder().encode(`v1|${KDF_LABEL}|${salt}`);

const b64 = (bytes: Uint8Array): string => btoa(String.fromCharCode(...bytes));
const unb64 = (text: string): Uint8Array<ArrayBuffer> => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));

async function aesKey(kdf: VaultKdf, password: string, salt: Uint8Array): Promise<CryptoKey> {
  const pw = new TextEncoder().encode(password);
  // WebCrypto takes ArrayBuffer-backed views; wasm-bindgen returns exactly that.
  const raw = kdf(pw, salt) as Uint8Array<ArrayBuffer>;
  try {
    // Non-extractable: once imported, the key cannot be read back out.
    return await crypto.subtle.importKey("raw", raw, "AES-GCM", false, ["encrypt", "decrypt"]);
  } finally {
    wipe(pw, raw);
  }
}

export async function sealVault(kdf: VaultKdf, mnemonic: string, password: string): Promise<VaultRecord> {
  if (password.length < MIN_PASSWORD) throw new Error(`password must be at least ${MIN_PASSWORD} characters`);
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await aesKey(kdf, password, salt);
  const plain = new TextEncoder().encode(mnemonic);
  try {
    const saltText = b64(salt);
    const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: aad(saltText) }, key, plain));
    return { v: 1, kdf: KDF_LABEL, salt: saltText, iv: b64(iv), ct: b64(ct) };
  } finally {
    wipe(plain);
  }
}

/** Opens a vault. A wrong password fails the GCM tag; the error says only that. */
export async function openVault(kdf: VaultKdf, record: VaultRecord, password: string): Promise<string> {
  if (record.v !== 1 || record.kdf !== KDF_LABEL) throw new Error("unknown vault format");
  const key = await aesKey(kdf, password, unb64(record.salt));
  let plain: Uint8Array;
  try {
    plain = new Uint8Array(
      await crypto.subtle.decrypt({ name: "AES-GCM", iv: unb64(record.iv), additionalData: aad(record.salt) }, key, unb64(record.ct)),
    );
  } catch {
    throw new Error("wrong password");
  }
  try {
    return new TextDecoder().decode(plain);
  } finally {
    wipe(plain);
  }
}
