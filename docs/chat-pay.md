# Paying in Maya Chat

Status: RESEARCH, unaudited (ADR-031, ADR-088). The protocol is in
`apps/chat`. The desktop Pay card and the wallet's receiving side are not
built yet; use test amounts on testnet only.

## What the protocol does

- **Ask for money.** A payee sends a request: an amount, a note (at most 140
  bytes of plain text), and an expiry no more than 24 hours away. The request
  is signed by the payee's chat identity, and it names only that payee's own
  account.
- **Pay.** The payer's wallet pays the account the request names. The wallet
  shows its own full confirmation, and the user approves there. Chat never
  signs a transaction.
- **Confirm.** The payer's chat sends a receipt: the request id and the
  transaction id, signed by the payer's chat identity.
- **Check.** The payee accepts the receipt only if the chain shows a
  transaction with that id paying the payee at least the amount. A receipt
  alone is never enough.

## What chat never does

- It never holds a wallet key or funds.
- It never sends money.
- A link from outside (a wallet return link) fills in a receipt. It never
  sends one on its own.

## Limits

- A request names only the account of the person who sends it.
- A note with control characters, line breaks or text-direction marks is
  refused.
- A payment request can be sent only in a chat where a message has already
  been exchanged. The first message in a new chat must be text.
- Not yet: refunds, partial payments, several payees, group requests, the
  operating system's link registration (ADR-083), and the desktop Pay card.

## Link formats

Exact formats are in ADR-088 decision 5 and `apps/chat/src/pay_link.rs`. The
wallet must accept what that file accepts and nothing more.
