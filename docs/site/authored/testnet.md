---
title: Join the testnet
description: Get the Maya Wallet, take free test coins from the faucet, and send a post-quantum-signed transaction on maya-testnet-1.
---

**maya-testnet-1** is Maya2C's public test network. Every transaction on it is
signed twice, with ML-DSA-65 and SLH-DSA, the two NIST post-quantum signature
standards, and both signatures must verify.

:::caution[Test coins only]
Testnet coins have **no value** and cannot be bought or sold. Anyone offering
to sell you Maya2C coins is not selling anything this project issued. The
network may be reset, and the software has not had an independent security
audit. Don't use it to protect anything that matters yet.
:::

## 1. Get a wallet

**Desktop:** download **Maya Wallet** for Windows, macOS or Linux from the
[releases page](https://github.com/EricWijesinghe/Maya2C/releases).
Check the file against `SHA256SUMS` on the same page. The installers are not
code-signed yet, so Windows and macOS will warn about an unknown publisher.

Open it, choose **Create wallet**, write down the 24 words, and copy your
**address**: 64 characters of hex. The wallet already points at this testnet
(`https://rpc.maya2c.dev/rpc`).

**Command line:** the same release has `l1-wallet`:

```bash
l1-wallet --keystore my.key generate          # prints your address
```

## 2. Take free test coins

<form id="faucet-form" class="faucet-form">
  <label for="faucet-address">Your address</label>
  <input id="faucet-address" name="address" autocomplete="off" spellcheck="false"
         placeholder="64 hex characters" required />
  <button type="submit">Send me test coins</button>
  <p id="faucet-result" role="status" aria-live="polite"></p>
</form>
<script src="/faucet.js" defer></script>

<style>
  .faucet-form { display: grid; gap: 0.5rem; max-width: 42rem; }
  .faucet-form input { font-family: var(--sl-font-mono); padding: 0.5rem; }
  .faucet-form button { justify-self: start; padding: 0.5rem 1rem; cursor: pointer; }
  .faucet-form [data-state="error"] { color: var(--sl-color-red); }
</style>

Each address and each network connection can take 200,000 test coins a day,
enough for about seven transfers at the Standard fee.
The same faucet from a terminal:

```bash
curl -X POST https://faucet.maya2c.dev/request \
  -H 'content-type: application/json' \
  -d '{"address":"<your address>"}'
```

## 3. Send a transaction

In Maya Wallet: **Send**, paste a recipient address, enter an amount, pick a
fee (Standard is the default), **Review**, **Sign**, **Send**.

From the command line:

```bash
l1-wallet --keystore my.key --rpc-url https://rpc.maya2c.dev/rpc balance
l1-wallet --keystore my.key --rpc-url https://rpc.maya2c.dev/rpc \
    send --to <recipient address> --amount 100
```

A transfer is about 13 KB, because of the two post-quantum signatures, and
the fee is charged per byte. At today's base fee the Standard fee is about
26,500 base units, so keep that in your balance.

## Network facts

| | |
|---|---|
| Chain id | `maya-testnet-1` |
| Consensus | DAG-BFT: blocks are final in about a second |
| Genesis state root | `419ea2e9eea80f3c31d542e8dbf7e409a82a3cc249a872a4e5ae0c6bbc5f88b0` |
| Genesis block | `c80cc217d0079367f35dc92a508ed4bee44bf9b72594dcdaa15471df5bbf57d1` |
| Public API | `https://rpc.maya2c.dev`: JSON-RPC at `/rpc`, REST at `/v1/…` ([API reference](/guides/api/)) |
| Faucet | `https://faucet.maya2c.dev` |

## Honest limits, today

- **One validator.** The network currently runs on a single validator, so
  if it stops, the chain stops until it is back. It is moving to an
  always-on server next, and it needs four validators run by different
  people before it can survive a failure.
- **No peer-to-peer joining yet.** Running your own node against this
  network, or becoming a validator, opens once the seed is on a server with a
  public P2P port. Registration for validators is built and tested; it waits
  on that.
- **Reset twice so far.** The first genesis halted at block 12,530 on
  2026-09-29 because of a consensus bug in how DAG-BFT counted work (fixed,
  ADR-035). On 2026-10-04 the network restarted again so that every
  transaction signature commits to this chain's genesis (ADR-036): a transfer
  signed for this testnet can never be replayed on mainnet, or on any other
  Maya2C network. Signatures made before that restart are invalid here, so
  use a wallet built after it. Balances from earlier chains do not exist on
  this one.
- **Mining pays nothing.** See [mining and validators](/guides/mining/).
