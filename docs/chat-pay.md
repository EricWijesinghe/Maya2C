# Paying in Maya Chat

Status: RESEARCH, unaudited (ADR-031, ADR-088). The protocol is in
`apps/chat`. The desktop Pay card and the wallet's receiving side are not
built yet; use test amounts on testnet only.

## What the protocol does

- **Ask for money.** A payee sends a request: an amount, a note (at most 140
  bytes of plain text), and an expiry no more than 24 hours away. The request
  is signed by the payee's chat identity, and it names only the wallet
  account the payee's wallet bound to that chat identity.
- **Bind once.** In the wallet, "Chat payment binding" signs a statement,
  with the account's own key: this chat identity may ask for payments to
  this account, on this chain, for up to 90 days (30 by default). Paste it
  into the chat. A newer binding replaces the old one; a revocation, sent
  to your contacts, makes them refuse requests from you until you bind
  again.
- **Pay.** The payer's wallet pays the account the request names. The wallet
  shows its own full confirmation, and the user approves there. Chat never
  signs a transaction.
- **Confirm.** The payer's chat sends a receipt: the request id and the
  transaction id, signed by the payer's chat identity.
- **Check.** The payee accepts the receipt only if the chain shows that
  transaction, sent by the payer's bound wallet, after the request was made, in a finalized
  block, paying the payee at least the amount. A receipt alone is never
  enough. The request and the receipt are both signed for one chain, and
  are refused on any other.
- **No double pay.** The payer's chat makes a wallet link only for a request
  it received, before it expires, and not after it has sent a receipt for it.

## What chat never does

- It never holds a wallet key, and never signs a transaction. Money goes
  from one wallet account to another; a chat identity's own address is
  never a receiving account (ADR-088 amendment 1).
- Your wallet refuses a chat payment link for another chain, and one it has
  already paid.
- It never sends money.
- A link from outside (a wallet return link) fills in a receipt. It never
  sends one on its own.

## Limits

- A request names only the wallet account bound to the person who sends it.
- A contact that never received your revocation accepts the old binding
  until it expires.
- A note with control characters, line breaks, text-direction marks or
  other invisible formatting characters is refused.
- At most 64 open requests per contact in each direction.
- A payment request can be sent only in a chat where a message has already
  been exchanged. The first message in a new chat must be text.
- Not yet: refunds, partial payments, several payees, group requests, the
  operating system's link registration (ADR-083), and the desktop Pay card.

## Link formats

Exact formats are in ADR-088 decision 5 and `apps/chat/src/pay_link.rs`. The
wallet must accept what that file accepts and nothing more.
