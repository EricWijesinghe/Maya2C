# Threat model: Maya2C Chat expression, anonymity and deletion (ADR-140)

**Status:** design-stage model for [ADR-140](../adr/ADR-140-chat-expression-and-privacy.md),
2026-11-13. Nothing it describes is built. It adds to the models in
[ADR-031](../adr/ADR-031-maya-chat.md), [ADR-074](../adr/ADR-074-chat-outbox-receipts-envelope-v2.md)
and [ADR-085](../adr/ADR-085-chat-v2-groups-on-mls.md); it does not replace them.
Self-written; no external review. The chat is RESEARCH and unaudited.

## Assets

| Asset | Why it matters |
|---|---|
| Message and media plaintext | the conversation |
| Who talks to whom, when, how much | often more sensitive than content |
| The link between a chat identity and a wallet address | money and speech joined |
| The link between a user's pairwise identities | defeats anonymity |
| The identity seed (OS keychain) | everything derives from it |
| Local history and caches | what a seized device yields |
| Audience lists, block lists, presence | the social graph |

## Adversaries

- **N — network observer:** ISP, Wi-Fi owner, national-scale passive tap.
- **R — relay:** honest-but-curious or malicious operator; may collude with N.
- **M — member:** a contact or group member, possibly running a modified client.
- **D — device seizure:** physical access, locked or unlocked; forensic imaging.
- **S — stranger:** spammer or harasser with unlimited free addresses.

Out of scope: a compromised operating system or malware on the device (it sees
what the user sees), and a break of ML-KEM and X25519 together.

## What each adversary learns

| Feature | N | R | M | D |
|---|---|---|---|---|
| Reactions, replies, edits, polls | size bucket, timing | same, plus recipient address | content (intended) | content until deleted |
| Stickers, custom emoji, notes | size bucket (larger) | same | content; the pack hash a sender uses, which is the same across chats | cached files until swept |
| Profile card | nothing | nothing | the card sent to them | all cards |
| Read receipts (opt-in) | timing | a small deposit shortly after a fetch | that it was read, to within the random delay | — |
| Typing (opt-in, direct only) | a direct connection between two IP addresses exists | nothing (never deposited) | activity | — |
| Presence / status (opt-in) | timing | deposits to each chosen contact at once: a fan-out burst | the status | the contact list it went to |
| Stories | timing | a burst of same-size deposits: audience size, not who | the story; not the rest of the audience | story until expiry; audience lists |
| Gifts | nothing new | nothing new | **the payer's wallet address, on chain, for everyone** | the gift history |
| Pairwise identity | relay IP reached | separate mailboxes; linkable by IP and timing, not by key | one address that matches nothing else | every label and identity |
| Disappearing / delete | nothing | an unfetched envelope expires | **whatever a modified client kept** | whatever was not yet swept |

## Threats and decisions

| # | Threat | Adversary | Mitigation in ADR-140 | Residual |
|---|---|---|---|---|
| T1 | Frame type guessed from ciphertext size | N, R | pad plaintext to buckets before sealing | large media stays distinguishable from text |
| T2 | Fan-out burst reveals a story or status audience size | R | jitter deposits across a window; same bucket | size is still roughly visible over time |
| T3 | Typing or presence gives a live activity trace | R | typing never deposited; presence opt-in and per contact; "last active" rounded | a direct connection reveals IP to the peer; the setting says so |
| T4 | Pairwise mailboxes linked by one connection | R, N | one connection per mailbox, independent timing | same IP; fixed only by a mix layer, which is not built |
| T5 | Pairwise identity linked to the wallet by a gift | M, public | warning before the wallet opens in incognito chats; stealth addresses (ADR-102) when they exist | a user who proceeds is linked, permanently |
| T6 | Badge links an anonymous identity to a person | M | badges unavailable on pairwise and incognito identities | none by design |
| T7 | Sticker or emoji pack hash acts as a cross-chat fingerprint | M | incognito mode offers only the bundled starter pack | a user who imports a rare pack into a private chat is recognisable by it |
| T8 | Forged delete request removes another person's messages | M | `Delete` accepted only from the author's session; in groups, author or admin | a malicious admin can delete; history shows nothing unless Q5 picks a tombstone |
| T9 | Forged reaction, edit or reply attributed to another member | M | frames are authenticated by the session (1:1) or by MLS sender data (groups) | none known |
| T10 | Modified client ignores timers and deletes | M | none possible; UI copy states it | full |
| T11 | Screenshot or camera | M | OS content-protection flags, best effort, labelled | full for a camera; partial for capture tools |
| T12 | Sender's clock used to extend or shorten a timer | M | receiver counts on its own clock from first display; backwards jumps never extend | a receiver who stops their own clock is T10 |
| T13 | Deleted data recovered from flash storage | D | per-chat keys; delete erases the wrapped key, then rewrites | key bytes may persist in old flash blocks until the OS reuses them; full-disk encryption is the recommended companion |
| T14 | Seized unlocked device | D | app lock with timeout; previews off when locked; short timers | an unlocked, open app shows everything on screen |
| T15 | Coerced unlock | D | duress passcode triggers panic wipe | the wipe is visible afterwards; identity is lost without a backup |
| T16 | Message-request flood | S | requests render no media or links; local rate rule; contacts-only switch; relay postage | CPU-rich attacker still fills the request list; requests are capped and oldest dropped |
| T17 | Malicious media (decoder exploit) in sticker, note, story or avatar | M, S | type sniffed from bytes, size and dimension caps, SVG only as `<img>`, Lottie light build without eval (all built for stickers); the same path for every new media kind; nothing from a stranger renders before accept | decoder bugs in the webview remain; keep it updated |
| T18 | Fake gift animation without payment | M | animation plays only after the recipient's own settlement check | none known; an unverified frame shows as text |
| T19 | Gift frame names a real transfer made by someone else | M | payer address of the transaction must be the one the #441 request and receipt bind | depends on #441's check holding |
| T20 | Abuse report used to frame someone | M | reports are plain forwards and carry no proof; recipients of a report are told so | a forged report is as credible as a screenshot; franking (Q7) would change that at the cost of deniability |
| T21 | Engagement mechanics pressure users into exposure | product | no streaks, no public counts, no synthetic notifications (ADR-140 §7) | none by design |
| T22 | Public channel mistaken for a private one | user | permanent "Public: anyone can read this" label; different visual treatment | user inattention |

## Red-team list for the build PRs

Each build PR adds the strongest of these as a test (Proof discipline 3).

1. Reaction, reply or delete naming an id from a different chat.
2. A `Delete` replayed after the author re-sent the same text.
3. A timer frame with `u64::MAX` or zero.
4. A story key reused across two stories.
5. Ten pairwise mailboxes fetched in one pass: what a logging relay can join.
6. A `Gift` frame for an unsettled, reorganised or third-party transaction.
7. A profile card with a 10,000-frame Lottie avatar under the byte cap.
8. Old client receiving every new frame kind.
9. Panic wipe interrupted halfway (power loss): what still decrypts.
10. Clock set back one year with timers running.

## Not verified

Everything. This is a design model; no code exists and no measurement was
taken. The mix layer it refers to has no ADR yet.
