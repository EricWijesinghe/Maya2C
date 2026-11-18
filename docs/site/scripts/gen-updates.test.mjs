// node --test scripts/gen-updates.test.mjs
// The asset-name mapping the update manifests depend on. A prefix that no
// longer matches drops an app's manifest, so its users silently stop getting
// updates: this pins the names the release job produces (spaces -> dots).
import { test } from "node:test";
import assert from "node:assert/strict";
import { appOf } from "./gen-updates.mjs";

test("the renamed wallet's assets map to the wallet manifest", () => {
  assert.equal(appOf("Maya2C.Wallet_0.1.1_x64-setup.exe.sig"), "wallet");
  assert.equal(appOf("Maya2C.Wallet_0.1.1_amd64.AppImage.sig"), "wallet");
  assert.equal(appOf("Maya2C.Wallet.app.tar.gz.sig"), "wallet");
});

test("releases from before the rename still map", () => {
  assert.equal(appOf("Maya.Wallet_0.1.0_x64-setup.exe.sig"), "wallet");
});

test("the other apps keep their manifests", () => {
  assert.equal(appOf("Maya.Chat_0.1.0_x64-setup.exe.sig"), "chat");
  assert.equal(appOf("Maya2C.Hub_0.1.0_x64-setup.exe.sig"), "hub");
});

test("a prefix must end at a separator, and unknown assets map to nothing", () => {
  assert.equal(appOf("Maya2C.WalletTool_1.0_x64-setup.exe.sig"), undefined);
  assert.equal(appOf("maya2c-node-x86_64-linux.tar.gz.sig"), undefined);
});
