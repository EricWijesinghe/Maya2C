# Chat mini-apps and the in-chat wallet

Status: **stub**. Not compiled or run on a desktop yet. Design: [ADR-138](adr/ADR-138-chat-mini-apps.md).

## What a user sees

- **Request** (₥ button in a chat): ask the other person for an amount in
  coins with an optional memo. They get a request bubble.
- **Pay** on a request bubble: a confirm box shows who asks, the full payee
  address, the amount and the memo. Only after **Yes** does a wallet link
  appear. The wallet shows the payment again and signs; chat never does.
- **I paid**: paste the link the wallet returns
  (`maya2c-chat://paid?request=…&tx=…`) to send a signed receipt.
- **Mini-apps** (▦ button): installed apps, each with its publisher and the
  permissions it asks for. An app opens in a sandboxed panel with no network.

## What an app can do

| Permission | The app may | The user is asked |
|---|---|---|
| `read-address` | read your address | once |
| `request-payment` | propose a payment to your wallet | once to allow, then every payment |
| `send-message` | send text in this chat, labelled `[App name]` | once |

Nothing else. No app can sign, read your messages or reach the internet.

## Making an app

1. Write one self-contained `index.html` (inline script and style; images as
   `data:` URLs).
2. Sign a manifest with `maya_chat::miniapp::Manifest::sign` (hybrid key from
   a 32-byte publisher seed), and write `Manifest::to_bytes()` to
   `manifest.cbor`.
3. Put both files in `<app data>/miniapps/<id>/`, `<id>` lowercase letters,
   digits and `-`.

Inside the app, talk to the host with `parent.postMessage`:

```js
parent.postMessage({ call: "address" }, "*");
parent.postMessage({ call: "pay", to: "tmaya1…", amount: "2500000000000000000000000", memo: "pizza" }, "*");
parent.postMessage({ call: "send", text: "Bill split: 2.5 each" }, "*");
window.addEventListener("message", (e) => console.log(e.data)); // { reply: "address", address } …
```

`amount` is base units (24 decimals). Replies: `address`, `confirmed`,
`sent`, `declined`, `refused` (with `why`).

## Tests

`apps/chat/tests/miniapp_tests.rs`: bad signature refused; permission outside
the allow-list refused; unknown origin dropped; payment needs the user's yes
(mock confirm); undeclared permission refused without a prompt; app text
cannot pose as a payment; bad pay fields refused before any prompt.
`apps/chat-gui/src-tauri/src/miniapp_cmds.rs` and `pay_cmds.rs` hold unit
tests for app ids, the app CSP and the request bubble.
