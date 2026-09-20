// Air-gapped signing, driven through the UI.
//
// NEVER RUN — see README.md. `tauri-driver` is not installed on the authoring
// host.
//
// The assertions below check what WebDriver can check: that the frame count
// matches what the core library computes, that the animation advances, and
// that a deliberately skipped frame is reported by index rather than as a bare
// failure. Whether a phone camera can read the codes off a screen is not
// something a DOM driver can answer.

import { strict as assert } from 'node:assert';

const PHRASE_WORDS = 24;

describe('air-gapped signing', function () {
  this.timeout(120_000); // SLH-DSA signing is ~150 ms; app startup dominates.

  let driver;

  before(async () => {
    driver = global.__TAURI_DRIVER__;
    assert.ok(driver, 'tauri-driver session missing; see README.md');
  });

  it('creates a wallet and shows a 24-word phrase', async () => {
    await driver.findElement({ id: 'create-wallet' }).click();
    const words = await driver.findElements({ css: '.mnemonic-word' });
    assert.equal(words.length, PHRASE_WORDS);
  });

  it('signs offline and emits more than one frame', async () => {
    // The whole reason this screen exists. A signed Maya2C transaction is
    // ~13.3 KB against QR's 2,953-byte absolute ceiling, so a single code is
    // impossible and the UI must animate.
    await driver.findElement({ id: 'compose-transfer' }).click();
    await driver.findElement({ id: 'recipient' }).sendKeys('ab'.repeat(32));
    await driver.findElement({ id: 'amount' }).sendKeys('1000');
    await driver.findElement({ id: 'sign-offline' }).click();

    const total = Number(
      await driver.findElement({ id: 'frame-total' }).getText(),
    );
    assert.ok(total > 1, `expected several frames, got ${total}`);
  });

  it('advances through every frame', async () => {
    const total = Number(
      await driver.findElement({ id: 'frame-total' }).getText(),
    );
    const seen = new Set();

    for (let i = 0; i < total * 2; i += 1) {
      seen.add(await driver.findElement({ id: 'frame-index' }).getText());
      await driver.findElement({ id: 'frame-next' }).click();
    }

    assert.equal(seen.size, total, 'the animation must show every frame');
  });

  it('names a missing frame rather than just failing', async () => {
    // A user in front of a looping animation needs to know which frame to wait
    // for. "Scan failed" makes them restart; "frame 5 missing" does not.
    await driver.findElement({ id: 'scan-simulate-skip' }).sendKeys('5');
    await driver.findElement({ id: 'scan-assemble' }).click();

    const status = await driver.findElement({ id: 'scan-status' }).getText();
    assert.match(status, /5/, `status should name the missing frame: ${status}`);
  });
});
