# ADR-140: Maya2C Chat expression, anonymity and deletion suite

**Status:** Proposed, 2026-11-13. Design only; nothing in this ADR is built.
Maya2C Chat stays RESEARCH and unaudited ([ADR-031](ADR-031-maya-chat.md)).
**Date:** 2026-11-13
**Related:** [ADR-031](ADR-031-maya-chat.md) (protocol), [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md)
(outbox, receipts), [ADR-085](ADR-085-chat-v2-groups-on-mls.md) (groups on MLS,
Proposed), [ADR-102](ADR-102-stealth-addresses-ml-kem.md) (stealth addresses,
Proposed), [ADR-104](ADR-104-unique-assets.md) (unique assets, Proposed),
ADR-088 chat payment requests (PR #441, on its branch), ADR-138 chat mini-apps
(PR #495, on its branch), [threat model](../threat-models/chat-expression.md),
[prior-art/p2p-chat.md](../prior-art/p2p-chat.md).

Nothing here touches the node, the wire format, consensus or state. The node
binary does not link `maya-chat`. The one place value moves (gifts) reuses the
payment-request flow of PR #441 unchanged.

## Context

The owner's brief (2026-11-13): people join a messenger to express themselves,
to show off, to feel safe, to be anonymous when they choose, and to take
messages back. Telegram-style stickers, smooth graphics, customization, gifts,
status and stories are what bring people in. Maya2C Chat should carry every
feature people expect from other messengers, plus new ones, and be the growth
engine for the ecosystem.

The constraint is the design already accepted: end-to-end encryption with
post-quantum keys, no company in the middle, relays that hold ciphertext, no
phone number. Most engagement features on other platforms are built on a
server that sees content or the social graph. Each one has to be re-derived so
that no relay learns more than it learns today.

### What exists today (read from the tree, 2026-11-13)

| Part | State | Where |
|---|---|---|
| 1:1 sessions: ML-DSA-65 identities, signed X-Wing prekeys, per-direction symmetric chains | built, unaudited | `apps/chat/src/{identity,session}.rs` |
| Store-and-forward relays, PoW postage, 256 KiB envelope cap, 30-day maximum TTL | built | `apps/chat/src/relay.rs` |
| Sender outbox, signed delivery receipts with global and per-contact opt-out | built | `apps/chat/src/{outbox,receipt}.rs`, ADR-074 |
| Stickers and packs: Lottie, TGS, WebM, WebP, PNG/APNG, GIF, SVG; 120 KB per file; packs up to 120 items; hash-named, type-sniffed, cached encrypted | built | `apps/chat-gui/src-tauri/src/stickers.rs`, `features.toml` `chat-stickers` |
| Desktop app: seed in the OS keychain, contacts and history encrypted under a seed-derived key | built | `apps/chat-gui/src-tauri/src/store.rs` |
| Payment requests and receipts (chat asks, wallet signs) | open PR #441 | `apps/chat/src/pay.rs` on `feat/chat-pay` |
| Mini-apps (signed manifest, sandboxed frame, no signing in apps) | draft PR #495 | on `feat/p45-chat-mini-apps` |
| Groups on MLS, attachments, multi-device | Proposed, not built | ADR-085 |
| Mixnet or onion routing | **not built**; no code in the tree | named as later work in ADR-031 |
| Delay-tolerant delivery | property only (idempotent envelopes with their own expiry); no Bundle Protocol layer | ADR-031 |
| Unique assets (collectibles) | draft PR #493, post-genesis, dark (`u64::MAX`) | ADR-104, ADR-135 |

Two facts shape everything below:

1. **A structured-message frame already exists.** Stickers travel as
   `MAGIC ‖ header_len ‖ JSON header ‖ file` inside the encryption, with a
   tagged `Content` enum. Reactions, replies, timers, deletion requests,
   stories and profile cards are new `Content` variants, not new protocols.
2. **Animated stickers are already built.** The remaining work is per-format
   caps, custom emoji and playback polish, not a new pipeline.

## 1. Feature matrix

Sources: each vendor's public pages, read 2026-11-13 and summarised (list at
the end). Rows marked † come from general product knowledge and were **not**
re-read on that date; treat them as unverified.

Status: **built** (in the tree), **planned** (an existing ADR or PR covers it),
**new** (this ADR).

| Feature | Who has it | Maya2C Chat | How it maps onto E2E and no central server |
|---|---|---|---|
| Static, animated and video stickers | Telegram (TGS 64 KB, WebM 256 KB, 3 s), others † | built (120 KB any format) | file inside the encrypted frame, named by its hash |
| Custom emoji | Telegram (100×100), Discord † | new | a sticker pack flagged `emoji`; rendered inline by hash |
| Reactions | all † | new | `Reaction` frame naming the target message id |
| Replies / quotes | all † | new | `Reply` frame carrying the target id and a short quote hash |
| Threads | Discord, Telegram topics † | new (groups only, after ADR-085) | a reply chain; no server index |
| Edit message | Telegram, Signal, WhatsApp, Threema (6 h window) | new | `Edit` frame signed by the same session; history kept locally unless the user hides it |
| Voice and video notes | all † | planned (needs ADR-085 D5 attachments) | chunked encrypted blob; short notes under the 120 KB cap can ship earlier |
| Polls | Telegram, WhatsApp, Signal, Threema | new (groups) | votes are group messages; tally computed by each client |
| Themes, wallpapers | Telegram, WhatsApp † | new | local settings; a shared chat theme is one frame |
| Animated profile frames, profile card | Telegram (premium) † | new | profile card sent per contact inside the session; no directory |
| Read receipts | all † | new, opt-in | `Read` frame; reciprocal; distinct from delivery receipts (built) |
| Typing indicator | all † | new, opt-in, off by default | ephemeral frame; see §2 for the leak |
| Presence / last seen | all † | new, opt-in | sent only to chosen contacts inside sessions; no relay learns it |
| Stories / status | Telegram, WhatsApp, Signal † | new | encrypted once, key fanned out over 1:1 sessions |
| Large groups | Telegram 200,000 (server-side encryption, not E2E); Session 100; SimpleX small groups | planned (ADR-085; cap unmeasured) | MLS; cap set by measured relay fan-out |
| Channels / communities | Telegram unlimited (not E2E); Session communities (not E2E) | new | signed broadcast; public channels are signed, not secret, and the UI says so |
| Gifts / stars | Telegram Stars (in-app currency, withdrawn as TON) | new, over #441 only | a tip is an ordinary transfer the wallet clear-signs; no new token |
| Mini-apps / bots | Telegram | planned (PR #495) | sandboxed, no signing |
| In-chat payments | Telegram, WhatsApp (some regions) †, iMessage (Apple Cash) † | planned (PR #441) | chat asks, wallet signs |
| No phone number | Session, SimpleX, Threema; Signal still needs one to register | built | identity is a key |
| No user identifier at all, per-contact addresses | SimpleX | new (pairwise identities) | per-contact key and mailbox |
| Incognito profile | SimpleX | new | random name per connection, over pairwise identities |
| Onion / mix routing | Session (onion requests) | planned, **not built** | later layer; honest label until then |
| Sender hidden from server | Signal sealed sender | built (the relay never learns the sender address; it still sees the depositing connection) | sender identity is inside the encryption |
| Disappearing messages | all (WhatsApp 24 h / 7 d / 90 d; Session after-send and after-read) | new | timer inside the frame; clients enforce |
| Delete for everyone | Telegram, WhatsApp †, Signal †, Threema (6 h) | new | signed request honest clients obey; plain limit in the UI |
| View-once media | WhatsApp, Signal, Telegram † | new | a timer of "after first view"; same limit |
| Screenshot notice or block | Telegram secret chats (best effort, by its own FAQ) | new, best effort | OS flags only; labelled |
| Safety numbers / key verification | Signal †, Threema (levels), Discord (verification code) | new | compare a code derived from both identity keys |
| App lock, panic wipe | Signal †, Threema †, Session † | new | OS passkey/biometric; key erasure |
| PQ key agreement | Signal (PQXDH, then SPQR with ML-KEM-768), iMessage PQ3, SimpleX (sntrup761) | built (X-Wing at handshake and re-key) | — |
| PQ continuous ratchet | Signal SPQR, iMessage PQ3 (about every 50 messages), SimpleX | planned (ADR-031 names it) | — |
| PQ **authentication** | iMessage PQ3 states its signatures are classical; not found in the others' pages read | built (ML-DSA-65 identities and handshakes) | see §8; no novelty claim |

## 2. Expression

All frames below are new variants of the structured `Content` header, inside
the encryption. A relay cannot tell a reaction from a text of the same padded
size. An older client that does not know a variant shows "This message needs a
newer Maya2C Chat" and nothing else.

**Message ids.** A frame that points at another message names the 32-byte
envelope id, the same id delivery receipts already carry (`receipt.rs`). PR 1
confirms the receiver can map that id to a stored line.

| Feature | Decision |
|---|---|
| Animated stickers | Keep the built pipeline. Add per-format caps beside the 120 KB total: Lottie/TGS ≤ 64 KB and ≤ 3 s at up to 60 fps; WebM ≤ 120 KB, ≤ 3 s, ≤ 30 fps, no audio track; one side 512 px. The Lottie and WebM figures mirror Telegram's published limits so existing artists' files fit; the 120 KB ceiling is ours, set by the 256 KiB envelope. Playback pauses off-screen and honours the OS reduced-motion setting. |
| Custom emoji | A pack with `kind = "emoji"`, items 100×100. Sent inline as `:hash:` references plus the file on first use per peer (the pack-request path already answers "send me the rest"). |
| Reactions | `Reaction { target, emoji }`; one per sender per target, replaced by a newer one, removed by an empty one. A custom emoji reaction carries its hash. |
| Replies, threads | `Reply { target, quote }` where `quote` is a BLAKE3 hash of the quoted text, so a reply cannot misquote; the app shows the local copy. Threads are reply chains in groups; they wait for ADR-085. |
| Voice and video notes | Notes that fit one envelope (≤ 120 KB, Opus or VP9) ship as a `Note` frame. Longer ones wait for the chunked blob store of ADR-085 D5. Metadata stripped before sealing. |
| Polls | Groups only. `Poll` and `Vote` frames; each client tallies. An anonymous poll is **not** offered until a design hides the voter from other members; a normal poll says "members can see how you voted". |
| Themes, wallpapers | Local by default. "Share this theme" sends one `Theme` frame (palette tokens from Forged Light 2 and an optional wallpaper file under the sticker cap). |
| Profile card, animated frame | `Profile { name, avatar, frame, accent }` sent to a contact inside the session, per contact, so different contacts can see different cards. No directory exists to scrape. The animated frame is a Lottie file under the sticker caps. |
| Read receipts | Opt-in, off by default, reciprocal (you see others' only if you send yours). A `Read` frame, delayed by the same random 0–30 s as delivery receipts (ADR-074 decision 3). |
| Typing indicator | Opt-in, off by default. Sent only over a direct peer connection when one exists; never deposited at a relay, because a deposit per keystroke burst would give the relay a live activity trace and cost postage. No direct connection: no indicator. |
| Presence / status | Opt-in, per contact list. A `Status { text, emoji, until }` frame sent inside sessions to the chosen contacts. "Online" is not broadcast; "last active" is rounded to the hour if shared at all. No relay holds a presence table. |
| Stories | See below. |
| Channels, large groups | See below. |

### Stories

- The story (image, short clip or text card) is sealed once under a fresh
  256-bit key. The key and the content hash go to each audience member over
  the existing 1:1 session.
- While the blob store of ADR-085 D5 does not exist, a story must fit one
  envelope and is fanned out as ordinary envelopes. Larger stories wait.
- **Expiry:** `expires_at` on the envelope (the relay drops it; `MAX_TTL` is
  already 30 days) and a 24-hour timer in the frame that honest clients obey.
  The same plain limit as §5 applies and the composer says so.
- **Audience lists** ("close friends", "everyone I talk to", custom) live on
  the author's device. Recipients cannot see who else received it.
- **Views:** a view receipt is opt-in on the viewer's side.
- No public story feed, no ranking, no autoplay into the next author (§7).

### Channels and large groups

- **Private groups** are ADR-085 groups. MLS key operations scale
  logarithmically, but delivery here is one deposit per recipient device, so
  the cap is a relay-budget figure. ADR-085 ships no number until measured and
  neither does this ADR. Size: **MISSING**.
- **Channels** are one-to-many. A private channel is an MLS group where
  members other than admins do not send. A **public channel** is signed
  posts under a channel key that anyone may fetch: authentic, not
  confidential, and labelled "Public: anyone can read this". We do not call a
  public channel end-to-end encrypted. Telegram's and Session's large public
  spaces make the same trade and are not E2E either.

## 3. Gifts

**Decision: a gift is a peer-to-peer MAYA tip with an animation, built only on
the payment flow of PR #441.**

- The sender picks a gift (animation plus amount). Chat builds the same
  `maya2c-wallet://pay?to=&amount=&request=` hand-off #441 defines. The wallet
  shows the full payee address, amount, memo and fee, and the sender
  **clear-signs** there. Chat never holds a wallet key.
- When the wallet returns `maya2c-chat://paid`, chat sends a `Gift` frame
  naming the transaction id and the animation hash. The recipient's app plays
  the animation only after its own check that the transaction settles
  (`settles_on_chain` in #441). A `Gift` frame without a settled transfer
  renders as plain text "unverified gift".
- **No new token. No protocol fee beyond the ordinary network fee. No
  platform cut.** There is no in-app balance, so there is nothing to top up or
  cash out.
- **Collectible, later:** once unique assets exist on chain (ADR-104, PR #493,
  post-genesis and dark today), a gift may also transfer an item to the
  recipient. It is a second ordinary transaction the wallet clear-signs. Not
  before that feature is scheduled.
- **Gift animations** are sticker-format files; anyone can make a gift pack.
  A pack is art, not a priced good.

**Flag for the owner and a lawyer.** Any of the following changes the legal
character of the feature (money transmission, stored value, consumer law,
app-store payment rules) and is not decided here: taking a cut; holding a
balance for users; selling gifts for fiat; a cash-out service; paid gift packs
sold by the project. Each needs Eric's decision and legal advice first.

**Privacy cost, stated plainly.** A tip is an on-chain transfer. It shows the
payer's and payee's wallet addresses to each other and to everyone. Sending a
gift from an anonymous pairwise identity (§4) links that conversation to a
wallet address unless stealth addresses ([ADR-102](ADR-102-stealth-addresses-ml-kem.md),
Proposed) are in use. The gift sheet in an incognito chat warns before
opening the wallet.

## 4. Anonymity

| Measure | Decision | Built today? |
|---|---|---|
| No phone or email, ever | Unchanged from ADR-031. No recovery flow may introduce one. | built |
| Pairwise identities | For a contact or group marked private, the app derives a separate ML-DSA-65 identity: `BLAKE3-derive("maya-chat pairwise identity v1", seed ‖ random 32-byte label)`. Its address, prekeys and mailbox are its own. It is not the wallet address and cannot be linked to it or to the user's other pairwise identities from keys alone. The label is stored in the encrypted profile. | new |
| Incognito profile | A mode over pairwise identities: random display name and avatar per connection, no profile card, receipts and presence forced off, gifts warn (§3). | new |
| Metadata minimisation | Pad every plaintext to size buckets before sealing (ADR-085 D8 already requires it for groups; extend to 1:1). Random delay on receipts (built). No contact upload, no directory, no server-side profile. | partly built |
| Mix or onion routing | **Not built.** No Sphinx or onion code exists in the tree. Until it does, the app must not say "anonymous on the network". Interim option the user can turn on: route relay connections through a local Tor SOCKS proxy if one is running (documented, not bundled). | planned |

**What pairwise identities do not hide.** One device polling ten mailboxes
from one IP address in one connection links them for the relay. PR 7 must
therefore fetch each pairwise mailbox on its own connection with independent
timing, and the residual (same IP, correlated timing) stays on the label until
a mix layer exists.

### Threat model summary

Full version: [threat-models/chat-expression.md](../threat-models/chat-expression.md).

| Adversary | Main identity | Pairwise identity | Incognito profile | With a mix layer (planned) |
|---|---|---|---|---|
| Network observer (ISP, Wi-Fi) | sees you reach a relay; sizes and timing | same | same | sees mix traffic only |
| Relay | recipient address, sizes, timing, depositing connection | cannot link pairwise mailboxes by key; can by IP and timing | same | loses IP and most timing |
| Group member or contact | your one address everywhere; can compare notes with others | a different address per contact; cannot compare by key | plus no name or avatar to compare | unchanged |
| Device seizure (unlocked or seed extracted) | everything, including all pairwise labels | everything | everything | unchanged |

Device seizure defeats every row. The defences are §5 (short timers, key
erasure) and §6 (app lock, panic wipe), not anonymity features.

## 5. Deletion, honestly

**The plain limit, which the UI must state where the feature is switched on:**

> Maya2C Chat asks the other person's app to delete. An app that has been
> modified, a screenshot, or a camera pointed at the screen can keep a copy.
> No messenger can prevent that.

| Feature | Decision |
|---|---|
| Disappearing messages | Per-chat timer (off, 30 s, 5 min, 1 h, 1 day, 7 days, 90 days) set by a `Timer` frame either side may send; it applies to later messages. Each message carries its timer in the header. The receiver counts from first display; the sender from send. The envelope's `expires_at` is capped so an unfetched message does not outlive its timer at the relay. A default timer for new chats is a local setting. |
| View once | A timer value of "after first view" for media. Same limit. |
| Delete for everyone | A `Delete { targets }` frame inside the session. An honest client removes the lines, any cached file, and the notification, and shows nothing in their place (no "message deleted" tombstone, unless Q5 decides otherwise). A user may delete only messages they sent; in groups an admin may also delete (after ADR-085). No time window. The request is authenticated by the session, which is what makes it "signed": no separate signature that could later prove what was deleted. |
| Edge cases the copy names | A reply's quote hash stays but its text is gone. A message already forwarded elsewhere is not reached. A device that is offline deletes when it next syncs, or the envelope expires first. |
| Local secure deletion | History moves from one blob under one key to **one random key per chat**, wrapped by the profile key. Deleting a chat erases its wrapped key, then rewrites the file. Disappearing messages are removed and the store compacted on each sweep. Overwriting on flash storage is not guaranteed by any app; erasing the key is the part that holds, and the docs say so. |
| Screenshot protection | **Best effort, labelled "may not work on every device".** Android `FLAG_SECURE`; Windows `SetWindowDisplayAffinity` (exclude from capture); macOS window `sharingType = none`. Tauri exposes these as content protection; exact coverage is verified in PR 6. Linux has no equivalent and the setting is hidden there. No screenshot *notification* is promised: Telegram's own FAQ says its detection can be bypassed. |

## 6. Safety

| Feature | Decision |
|---|---|
| Block | Local list. Envelopes from a blocked session are dropped unread; no receipt, no signal to the sender. For a stranger flood, "contacts only" refuses handshakes from unknown addresses. |
| Mute | Local, per chat, with duration. |
| Spam filtering | On the device only. First contact arrives as a **message request** showing no media, no links and no stickers until accepted. Rules (rate of new requests, known-bad pack hashes) run locally. Relay postage (built) stays the network-level cost. A proof-of-personhood stamp (`crates/personhood`, RESEARCH) remains its own later decision, as ADR-031 says. |
| Safety numbers | A 60-digit code and QR derived from both identity public keys with BLAKE3. Shown per contact; a key change raises a visible warning and clears the "verified" mark. |
| Panic wipe | A gesture or a duress passcode deletes the keychain seed entry, then the files. Everything left on disk is ciphertext without a key. It also destroys the identity unless the user kept a backup, and the setup screen says so. |
| App lock | OS passkey or biometric (Windows Hello, Android BiometricPrompt, Touch ID) gates unwrapping of the profile key; a timeout re-locks. Notification previews off while locked. |
| Abuse reporting without breaking E2E | Reports are made by the recipient, who already holds the plaintext: "Report" forwards the chosen messages to whoever the user picks (a group admin, a relay operator's abuse address). Nobody scans content, and no key is escrowed. |
| Message franking (option, not decided) | Franking lets a recipient *prove* to a third party what a sender wrote, using a committing tag bound at send time (Facebook's scheme; "asymmetric message franking" for systems that hide the sender). It needs a committing construction, which ChaCha20-Poly1305 alone is not, and it removes deniability for reported messages. With no central moderator to receive proofs, its value here is limited to group admins. Q7. |

## 7. Engagement, ethically

A private messenger sells one thing: the user's belief that it is on their
side. A mechanic that works against the user spends that belief. So:

**Build (bonding):**

- **Shared memories:** "a year ago in this chat", computed on the device from
  local history. Never for a chat with a timer.
- **Anniversaries:** the day a contact was added, or a date the two people
  set together.
- **Co-created sticker packs:** two or more people add to one pack; the pack
  format already identifies a pack by its contents, so a shared pack is a
  sequence of versions.
- **Gift moments:** a gift animation that plays for both people once the
  transfer settles (§3).
- **Chat themes chosen together** (§2).

**Do not build:**

- No streaks, and nothing that is lost by not opening the app.
- No infinite or autoplaying feed; stories stop after the author you opened.
- No notification that is not a real event from a real contact. No "people
  you may know", no re-engagement nudges.
- No public counts designed for comparison (follower totals on profiles,
  gift leaderboards).
- No variable-reward mechanics (loot boxes, random gift contents).

Every notification maps to a message, call, gift or request the user can see.
That is a testable rule (PR 9 acceptance).

## 8. Ideas beyond the benchmark set

None of these is claimed as new to the world. Each line names who does
something similar.

| Idea | What it is | Similar work |
|---|---|---|
| Post-quantum authentication as the headline, beside post-quantum secrecy | Identities and handshakes are signed with ML-DSA-65 today. | Signal, iMessage and SimpleX ship PQ key agreement; Apple states PQ3 signatures are classical. [prior-art/p2p-chat.md](../prior-art/p2p-chat.md) lists this as a candidate difference that is **not yet verified**; it stays unclaimed until that search is done. |
| On-chain verifiable badges, opt-in | A contact can show a badge backed by a selective-disclosure credential (`crates/identity`, `docs/identity.md`): "over 18", "member of X", "holds name Y" (names: ADR-105, Proposed). The proof is checked on the verifier's device; no claim preimage is on chain. Never available on a pairwise or incognito identity, because a badge is a link. | Verified badges on centralised platforms (operator-issued); ENS names shown in web3 messengers such as XMTP-based apps and Status. |
| Key-change transparency through the chain | Rotation and revocation of a chat identity anchored on chain (already named in ADR-031), so a contact's app can check a key change against a public record. | Key transparency logs at WhatsApp, Signal and Apple (operator-run logs). |
| Verifiable gifts | The animation plays only after the recipient's own app confirms settlement. | Telegram gifts (operator ledger). |
| Two-person vault chat | A chat whose history key is split so that both people must be present to open old messages. Sketch only; needs its own ADR. | Secret sharing in password managers' emergency access. |
| Self-hosted relay as a one-click setting | Any user can run a relay; the app can make "use my own" one switch. | SimpleX and Matrix self-hosting. |
| On-device translation in E2E chats | Already catalogued as `babel-chat` with its own prior-art record. | [prior-art/babel-chat.md](../prior-art/babel-chat.md). |

**On the marketing line.** "The most decentralized private chat" is a
superlative. Standing Order 10 applies in spirit even though the checker's
word list does not catch it: the line may be used only once
`docs/prior-art/` records a dated comparison that supports it. Until then the
copy describes facts: no phone number, no central server, post-quantum keys,
run your own relay.

## 9. Build order

Small PRs, in the order that reuses built parts most. Each is tests first.
None touches `crates/node`. PRs 1–6 need only what is on master. PR 8 is
stacked on #441. Every PR adds its `Content` variant to a shared
unknown-variant test so older clients degrade to the "needs a newer version"
line.

| # | PR | Files | Tests to write first | Accepted when |
|---|---|---|---|---|
| 1 | Reactions and replies | new `apps/chat/src/frame.rs` (the `Content` frame moves from chat-gui so the CLI and GUI share it; stickers.rs re-exports), `apps/chat-gui/src-tauri/src/store.rs` (`Line` gains id, reactions, reply), `ui/app.js` | frame round-trips; unknown variant is refused cleanly; a reaction to an unknown id is dropped; a second reaction from one sender replaces the first; a reply whose quote hash mismatches shows no quote | two clients react and reply over a relay in an e2e test; padded sizes of a reaction and a short text are equal |
| 2 | Disappearing timers | `apps/chat/src/frame.rs` (`Timer`, per-message timer), new `apps/chat-gui/src-tauri/src/expiry.rs`, `store.rs` | timer applies only to later messages; sweep removes a line and its cached file at the deadline; envelope `expires_at` never exceeds the timer; a clock moved backwards does not extend a message's life | a message with a 30 s timer is gone from both test clients' stores and the UI copy shows the plain limit |
| 3 | Sticker caps and custom emoji | `stickers.rs`, `ui/stickers.js` | each per-format cap refuses one byte, frame or millisecond over; WebM with an audio track is refused; an emoji pack item over 100×100 is refused; reduced-motion shows the first frame | Telegram-sized TGS and WebM samples made for the test import; emoji render inline |
| 4 | Profile card and themes | `frame.rs` (`Profile`, `Theme`), `store.rs`, `ui/` | a card is sent per contact and never to a blocked one; two contacts can hold different cards; an oversized avatar or frame is refused; theme tokens outside the allowed set are ignored | theme and card survive restart; no card leaves the device without a contact chosen |
| 5 | Stories (single-envelope) | new `apps/chat/src/story.rs`, `apps/chat-gui/src-tauri/src/story_cmds.rs`, `ui/stories.js` | a non-audience contact receives nothing; the key differs per story; expiry removes it from store and cache; a recipient cannot learn the audience; view receipts are off unless the viewer opted in | a story reaches exactly the chosen audience in an e2e test and is gone after expiry |
| 6 | Delete for everyone, per-chat keys, screenshot flag | `frame.rs` (`Delete`), `store.rs` (per-chat keys, version bump that reads the old profile), `apps/chat-gui/src-tauri/src/lib.rs` | only the author's messages are deleted; a `Delete` from the wrong session is ignored; deleting a chat erases its wrapped key and the old bytes do not decrypt; old profile version migrates; content protection is requested on supported platforms | red-team test: a forged delete for someone else's message is refused; UI shows the plain limit and the best-effort label |
| 7 | Pairwise identities and incognito | `apps/chat/src/identity.rs` (derivation), `client.rs` (several identities), `net.rs` (one connection per mailbox), `store.rs` | two pairwise addresses share no key material and differ from the main address; restore from seed plus labels recovers them; incognito sends no card, receipt or presence; mailboxes are fetched on separate connections | a test relay that logs everything cannot link two pairwise mailboxes by any field it stores; the residual (IP, timing) is in the docs |
| 8 | Gifts | `frame.rs` (`Gift`), `apps/chat/src/pay.rs` reuse, `ui/gifts.js` | no animation without a settled transfer; a `Gift` naming someone else's transaction is refused; amount shown equals the amount the wallet signed; incognito chat warns first | a testnet tip plays on both sides after settlement; grep shows no fee, cut or balance code |
| 9 | Safety pack | new `apps/chat-gui/src-tauri/src/safety.rs` (block, mute, requests, safety number, app lock, panic wipe) | blocked sender produces no receipt; a request shows no media before accept; the safety number is symmetric and changes with either key; panic wipe leaves no decryptable file; every notification type maps to a real event | red-team: a flood of 1,000 requests leaves the app usable |
| 10 | Read receipts, presence, typing | `frame.rs`, `client.rs`, `net.rs` (direct-only typing) | all three off by default; non-reciprocal peers see nothing; a typing frame is never deposited at a relay | relay log in an e2e test contains no presence or typing deposits |
| later | Voice/video notes over blobs, polls, threads, channels, badges, collectible gifts, mix layer | wait for ADR-085, ADR-104/#493, ADR-105 and a mixnet ADR | — | — |

Sizes are not estimated; no schedule is promised here.

## 10. Open questions for Eric

Recommended option in **bold**.

1. **Gift money rules.** (a) **Pure peer-to-peer tip, no cut, no balance, no
   fiat (this ADR)**; (b) add a cut or paid packs, after a lawyer's advice;
   (c) no gifts until after mainnet.
2. **Default disappearing timer for new chats.** (a) **Off, with a prompt on
   the third chat created**; (b) 7 days by default; (c) off, no prompt.
3. **Who may delete in a 1:1 chat.** (a) **Only your own messages**; (b)
   either side may delete any message for both, as Telegram allows; (c) own
   messages, plus "clear whole chat for both" if the other side accepts.
4. **Pairwise identities.** (a) **Opt-in per contact, and always on in
   incognito mode**; (b) on for every new contact; (c) not now.
5. **Tombstone after delete-for-everyone.** (a) **None**; (b) "a message was
   deleted", as WhatsApp shows.
6. **Read receipts and typing.** (a) **Both off by default, reciprocal**; (b)
   read receipts on by default; (c) do not build typing at all.
7. **Message franking.** (a) **Defer; ship user-side forwarding reports**;
   (b) build franking for groups with ADR-085; (c) never, to keep deniability.
8. **Public channels.** (a) **Build after groups, labelled "public, not
   secret"**; (b) do not build public spaces; (c) build before groups as
   signed broadcast.
9. **Network anonymity before a mix layer.** (a) **Document the optional
   local Tor proxy and say "not anonymous on the network" in the app**; (b)
   bundle Tor (larger download, store-review risk); (c) start the mixnet ADR
   now, ahead of groups.
10. **Marketing line.** (a) **Fact copy now; "most decentralized" only after
    a dated prior-art comparison**; (b) commission that comparison at once.

## Consequences

- Ten more frame kinds share one header format, so one fuzz target and one
  unknown-variant rule cover them.
- The history store changes shape once (per-chat keys, PR 6); the migration
  must read the old profile.
- Pairwise identities multiply mailbox polling. The cost is measured in PR 7
  before any default changes.
- Several features wait on ADR-085, which is still Proposed.
- The app gains settings whose honest labels are less attractive than
  competitors' ("best effort", "asks the other app"). That is accepted.

## Not verified

- Tauri content-protection coverage per platform (PR 6 checks it).
- That the receiver can always map an envelope id to a stored line (PR 1).
- Any group or channel size. MISSING until measured.
- Rows marked † in the matrix.
- Signal's Stories help page returned HTTP 403 on 2026-11-13 and was not read.
- No external review of any of this; the chat is RESEARCH.

## Sources (read 2026-11-13, summarised)

- Telegram stickers and emoji: https://core.telegram.org/stickers
- Telegram FAQ: https://telegram.org/faq (first 100,000 characters)
- Telegram Stars: https://core.telegram.org/api/stars
- Signal PQXDH: https://signal.org/blog/pqxdh/ ; SPQR: https://signal.org/blog/spqr/
- Signal usernames: https://signal.org/blog/phone-number-privacy-usernames/
- Signal sealed sender: https://signal.org/blog/sealed-sender/
- WhatsApp disappearing messages: https://faq.whatsapp.com/673193694148537
- Discord DAVE: https://discord.com/blog/meet-dave-e2ee-for-audio-video
- Session FAQ: https://getsession.org/faq
- SimpleX overview: https://simplex.chat/docs/simplex.html ; chat profiles: https://simplex.chat/docs/guide/chat-profiles.html ; v5.6: https://simplex.chat/blog/20240314-simplex-chat-v5-6-quantum-resistance-signal-double-ratchet-algorithm.html
- Threema FAQ: https://threema.com/en/faq (first 100,000 characters)
- iMessage PQ3: https://security.apple.com/blog/imessage-pq3/
