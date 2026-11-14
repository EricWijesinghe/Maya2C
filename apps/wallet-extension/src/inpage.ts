/**
 * `window.maya2c`: the page-facing provider (main world).
 *
 * Shaped after EIP-1193 — one `request({ method, params })` returning a
 * promise, errors carrying a numeric `code` — so dapp code ports, but the
 * methods are Maya2C's own:
 *
 *   connect        ask the person to let this origin see account 0
 *   accounts       the accounts this origin may see ([] when not connected)
 *   chain          { genesis, network, name } of the connected chain
 *   sendTransfer   { to, amount (base units, decimal string), genesis }
 *   disconnect     withdraw this origin's grant
 *
 * `sendTransfer` requires the genesis the dapp expects: the wallet refuses a
 * request for one chain while it is connected to another.
 */
import { createProvider } from "./lib/provider.js";

const provider = createProvider(window);
Object.defineProperty(window, "maya2c", { value: provider, writable: false, configurable: false });
window.dispatchEvent(new Event("maya2c#initialized"));
