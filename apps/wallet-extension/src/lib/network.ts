/**
 * Which network a gateway's node is, decided by its genesis id, and whether an
 * address may be paid on it. Mirrors `apps/wallet-gui/core/src/networks.rs`.
 *
 * The rule the extension adds is the #446 lesson: an address written for a
 * test network (`tmaya1…`/`dmaya1…`) is never paid from a mainnet chain, and a
 * mainnet address (`maya1…`) is never paid with test coins the payee would
 * read as real.
 */
import { decodeAnyAddress, encodeAddress, type AddressNetwork } from "../../../../sdks/sdk-js/src/address.js";
import { fromHex } from "./bytes.js";

/** Pinned mainnet genesis ids. Empty until the ceremony, as on the desktop. */
export const MAINNET_GENESIS: readonly string[] = [];
/** Pinned public testnet genesis ids. Empty, as on the desktop. */
export const TESTNET_GENESIS: readonly string[] = [];
const MAINNET_NAMES = ["maya-mainnet", "mainnet"];

export type NetworkClass = "mainnet" | "test" | "impostor";

export function classify(claimedName: string | null | undefined, genesis: string): NetworkClass {
  const g = genesis.trim().toLowerCase();
  if (MAINNET_GENESIS.some((p) => p.toLowerCase() === g)) return "mainnet";
  const claimsMainnet = MAINNET_NAMES.includes((claimedName ?? "").trim().toLowerCase());
  return claimsMainnet ? "impostor" : "test";
}

export function displayName(genesis: string): string {
  const g = genesis.trim().toLowerCase();
  if (MAINNET_GENESIS.some((p) => p.toLowerCase() === g)) return "Maya2C mainnet";
  if (TESTNET_GENESIS.some((p) => p.toLowerCase() === g)) return "Maya2C testnet";
  return "UNKNOWN NETWORK";
}

/** First and last eight hex characters, for comparing with a published id. */
export function fingerprint(genesis: string): string {
  const g = genesis.trim();
  return g.length <= 16 ? g : `${g.slice(0, 8)}…${g.slice(-8)}`;
}

/**
 * The Bech32m prefix addresses are written in on the chain `genesis`, by the
 * pinned lists alone: the desktop's `networks::address_network` and
 * sdk-wasm's `address_network`, so the account screen and the confirm screen
 * (rendered in wasm) never show one account under two prefixes.
 */
export function addressNetwork(genesis: string): AddressNetwork {
  const name = displayName(genesis);
  return name === "Maya2C mainnet" ? "mainnet" : name === "Maya2C testnet" ? "testnet" : "devnet";
}

export function displayAddress(genesis: string, id: Uint8Array): string {
  return encodeAddress(addressNetwork(genesis), id);
}

/** A recipient decoded and checked against the network it will be paid on. */
export interface Recipient {
  id: Uint8Array;
  /** The network the text named, or null for a bare hex id. */
  named: AddressNetwork | null;
}

/**
 * Parses a recipient and refuses one written for another network.
 * Hex ids carry no network; they are accepted and the confirm screen says so.
 */
export function parseRecipient(text: string, cls: NetworkClass): Recipient {
  if (cls === "impostor") throw new Error("this node claims to be mainnet with an unpinned genesis; refusing");
  const t = text.trim();
  if (/^(0x)?[0-9a-fA-F]{64}$/.test(t)) return { id: fromHex(t), named: null };
  const { network, id } = decodeAnyAddress(t);
  const ok = cls === "mainnet" ? network === "mainnet" : network !== "mainnet";
  if (!ok) {
    throw new Error(
      `this address is for ${network}, but the connected chain is ${cls === "mainnet" ? "mainnet" : "a test network"}; refusing to pay it`,
    );
  }
  return { id, named: network };
}
