// Loads the real sdk-wasm build for the tests. No mock: a test of crypto glue
// against a stand-in would pass while the shipped bytes were wrong. A missing
// build fails the suite rather than skipping it.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { initSync } from "../wasm/maya_sdk_wasm.js";
import * as wasm from "../wasm/maya_sdk_wasm.js";

const bytes = readFileSync(fileURLToPath(new URL("../wasm/maya_sdk_wasm_bg.wasm", import.meta.url)));
initSync({ module: bytes });

export { wasm };
