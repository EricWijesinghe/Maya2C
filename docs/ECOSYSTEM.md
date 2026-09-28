# The ecosystem plan

Eric's product sequence: **chat, then wallet, then AI services, then
others.** This is a plan, not a build order for the chain: it sits at M9 on
the [milestone ladder](EMPIRE.md), with one exception, chat, below.

Status 2026-09-28: Maya Chat's first version is being built in another
working session on branch `feat/utopia-chat` (`docs/adr/ADR-031-maya-chat.md` and
`reports/32-chat.md` on that branch; not yet on `master`). This file does not repeat that design; it sets
the rules every app shares and the questions each must answer.

## 1. The shared foundation

Apps stay **separate programs**. They share libraries and identity, never a
single monolithic app, so a flaw or a store rejection in one does not take
the others down.

| Shared piece | What it is | Where it comes from |
|---|---|---|
| Identity | One key per person, whose address is the chain's `suite_address`; later a `did:maya2c` document and the MP22 smart account that can rotate or recover it | `crypto-pq` suite registry; MP22 |
| Key custody | Keys generated and held on the device, zeroized, never sent to a server; recovery through the smart account's guardians, not a company | MP22, Standing Order 5 |
| Design system | One set of components and tokens | MP29 |
| SDK | One `maya-sdk` for signing, addresses, and (later) chain calls | MP09 SDKs |

Rule: an app may depend on `crypto-pq` and the SDK. It may not depend on
the node (`custom-l1-node`). Chat already follows this (ADR-031).

## 2. App 1 — peer-to-peer chat (the entry point)

**Who it is for.** People who need private messaging without handing a phone
number or address book to a company: activists, journalists, people in
places where messaging is watched, and people who simply want that.

**The one thing it must do better.** Not yet answered honestly. The prior-art
search ([p2p-chat.md](prior-art/p2p-chat.md)) found that post-quantum
end-to-end encryption, the candidate difference, already ships in Signal,
iMessage and SimpleX. The remaining candidates are post-quantum
*authentication* and a chat identity that is also a chain account; both need
their own dated search. Per this plan's rule, **an app with no honest answer
here is not started**. The build in progress is Eric's decision to make with
that finding in front of him (STATE.md, "Blocked on Eric").

**How success is measured.** Weekly active senders; messages delivered /
messages sent (target set after the first measurement, not before); median
delivery time for an offline recipient; battery and data per day on a
low-end Android phone. None has been measured.

**It must work without the blockchain.** Chat ships and gets users while the
chain is on testnet; chain-backed features (anchored key rotation, paying
for relay storage, wallet hand-off) arrive later.

### Hard problems it must design for explicitly

| Problem | The question to answer in a design doc | Status |
|---|---|---|
| Offline delivery | Who holds a message while the recipient is away, and what does that party learn? | ADR-031: store-and-forward relays see ciphertext, size, timing, recipient address |
| Push on iOS/Android | How does a phone wake without Apple/Google or a relay reading messages? Content-free push via the platform services is the usual answer; it still leaks timing to them | Open |
| Contact discovery | How do people find each other without uploading address books? QR/link exchange first; anything automatic needs a privacy design | Open |
| Cheap phones | Battery and data budget per day; ML-KEM/ML-DSA sizes are kilobytes per handshake | Open; to measure |
| Metadata privacy | Relays see who receives and when. Mixnets or onion routing are later decisions | ADR-031 names it as later |
| App-store policy | Apple and Google rules for P2P and crypto apps: no token or payments in v1 keeps chat inside ordinary messaging rules | Open |

### Abuse policy (a product requirement, written before launch)

End-to-end encryption means nobody but the participants can read content,
including us. So the policy works on what the system *can* see and do:

1. **Spam and floods.** Proof-of-work postage per message, adjustable per
   relay (ADR-031, revised after review); contacts-only mailboxes as an
   option; a personhood stamp later.
2. **Scams.** Clear warnings on first contact from an unknown identity; the
   wallet hand-off (App 2) always shows the full transaction, never a link
   that pays silently.
3. **Illegal content.** Users can report a conversation by *choosing* to
   forward the messages in it; a relay operator can refuse to serve an
   address; relays follow the law where they run. We do not add client-side
   scanning or any backdoor: that would break the security promise for
   everyone.
4. **Who decides.** Relay operators decide what they carry; the published
   policy says so. This needs Eric's sign-off and, before public launch, a
   lawyer's review — the legal risk is in RISKS.md.

## 3. App 2 — wallet

A separate app that the chat app hands off to for anything involving money:
chat asks, the wallet shows the full transaction (clear signing, MP22) and
signs. Chat never holds funds. For: chat users who get paid or pay. Better
than today: the same post-quantum identity for talking and paying, with
recovery that does not depend on a seed phrase on paper. Measured by:
successful hand-offs, time from request to signed payment, recovery drills
passed. Builds on the MP09/MP29 wallet, which exists.

## 4. App 3 — AI features

Before any AI feature is built, a one-page statement of **what data leaves
the device**, when, to whom, and how a user turns it off. Default: nothing
leaves the device; on-device models first; a remote model only with an
explicit per-conversation opt-in. Who it is for, the one thing it does
better, and how success is measured are to be answered in that statement.

## 5. Later services

Each is its own app on the shared identity, and each must first answer the
three questions: who it is for, the one thing it does better than what
those people use today, and how success is measured. Field extensions
already discussed (telecom, governance, others) wait for M9.
