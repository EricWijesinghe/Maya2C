/**
 * Content script (isolated world): relays `window.maya2c` calls from the page
 * to the background and back. It adds nothing to a message but its method and
 * params; the origin the background acts on is the one the browser stamps on
 * this script's messages, never one the page claims.
 */
import { installRelay } from "./lib/relay.js";

declare const chrome: any;
const api: any = (globalThis as any).browser ?? chrome;

installRelay(window, (msg) => Promise.resolve(api.runtime.sendMessage(msg)));
