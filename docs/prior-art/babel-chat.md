# Prior art: on-device translation inside end-to-end encrypted chat (babel-chat)

**Search status:** searched 2026-11-10 (partial: press and secondary sources only; Signal not found). Web searches by an agent
(CATALOG §I row `babel-chat`). Most sources are press or secondary coverage;
no vendor security whitepaper was read. Signal's position was not found. This
file authorizes **no** "first", "only" or "unprecedented" claim.

## What exists today

| Project | What it does | Where it runs | Source quality |
|---|---|---|---|
| WhatsApp message translation | Per-message and (Android) whole-thread translation, text only; language packs downloaded once, then offline. iOS 19 languages, Android 6 at launch. Meta states translation is on-device and E2EE is kept. On iPhone it uses Apple's translation API; on Android its own engine | On device (vendor statement) | Press coverage of the vendor statement; no independent audit found |
| Apple Live Translation (iOS 26) | Translation in Messages, FaceTime and Phone; Apple-built models; needs Apple Intelligence and recent hardware | On device per most sources; one low-quality source mentions a cloud fallback (disputed) | Press; not verified against Apple platform docs |
| Signal | Nothing found about translation | n/a | Absence in one search, not proof of absence |

## How a Maya2C version would differ (verified in the tree)

- Maya Chat's identity and handshake are post-quantum (ML-DSA-65, ML-KEM-768
  transport): `docs/adr/ADR-031-maya-chat.md:19,33,40`. That is a property of
  the chat, not of translation, and Signal and iMessage already ship post-quantum
  key agreement (see `docs/prior-art/p2p-chat.md`).
- Translation does **not exist** in the chat today: `rg -i "translat|on-device"`
  over `docs/adr/ADR-031-maya-chat.md`, `docs/adr/ADR-074-chat-outbox-receipts-envelope-v2.md`
  and `apps/chat/` returns no feature hits. `on-device-assistant` is a catalog
  row (`docs/ecosystem/CATALOG.md:100`), W3, not code.
- Candidate difference, **unbuilt**: a written, testable data-leaves-device
  statement (CATALOG row 100's gate) plus open-weight models whose hashes are
  pinned. Nothing in the tree implements or tests this, so it is a design
  intent, not a difference today.

## Legal and ethical gates

- Data-leaves-device statement must be true and tested (egress check in CI), or
  the feature ships off.
- Model licences (open weights) and per-language quality disclosure; machine
  translation errors in legal, medical or safety messages need a visible
  "machine translated" marker.
- Output language packs add download size and a model-supply-chain risk (ADR-080
  provenance would have to cover them).

## What may be said publicly

- Accurate: "Maya Chat has no translation feature today; on-device translation is a planned catalog item."
- **forbidden**: "first translating chat", "only private translation", "first on-device translation", "unprecedented", "world's first".

## Verdict

Similar exists, shipped at scale — differentiate by a verifiable no-egress
guarantee and pinned model provenance, both unbuilt. No difference is claimable today.

## Sources (retrieved 2026-11-10)

- https://www.macrumors.com/2025/10/08/whatsapp-rolls-out-message-translation/
- https://www.igeeksblog.com/whatsapp-message-translation-iphone-android/
- https://www.macrumors.com/2025/06/09/apple-live-translation-messages-facetime-phone/
- https://appleinsider.com/articles/25/06/11/new-ios-26-translation-tool-works-inside-calls-chats-video

