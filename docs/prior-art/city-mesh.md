# Prior art: City mesh (phones keep messaging and paying when the network is down)

**Search status:** searched 2026-11-10 (web search and page fetches; sources below). goTenna's security design and Meshtastic's firmware audits were not checked.

Feature: CATALOG §I row `city-mesh`.

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Briar | No central server; syncs over Bluetooth, Wi-Fi, memory cards or Tor, end-to-end encrypted; Cure53 audit 2017 | [How it works](https://briarproject.org/how-it-works/), [OTF audit](https://www.opentech.fund/security-safety-audits/briar/) |
| Meshtastic | LoRa mesh for text and position; default channel uses a well-known key; routing headers cleartext | [Overview](https://meshtastic.org/docs/overview/) |
| goTenna | Off-grid mesh radios and apps; now government/public-safety focused | [gotenna.com](https://gotenna.com/) |
| Bridgefy | Phone-to-phone Bluetooth mesh; broken by Albrecht et al. (CT-RSA 2021, USENIX Security 2022) | [ePrint 2021/214](https://eprint.iacr.org/2021/214), [USENIX 2022](https://usenix.org/conference/usenixsecurity22/presentation/albrecht) |

None of these moves payments on a base-layer ledger. Not found: an offline-payments mesh with post-quantum signatures (search not exhaustive).

## How a Maya2C version would differ (each backed by the repo)

- **Name collision:** `crates/mesh-api` is the Rosetta "Mesh" exchange API (`crates/mesh-api/src/lib.rs:1-5`), not a radio mesh.
- **Today's off-grid piece is a header relay:** `hal/radio-transport` carries headers and SPV proofs, "not transactions" (`lib.rs:1-5`); a transaction is ~13 KB = 60 frames at SF7, 259 at SF12 (`lib.rs:21`); at 1% duty cycle ~40 minutes of silence at SF7 (`lib.rs:22-25`). Paying over this mesh is not possible as built.
- **No phone-to-phone transport exists** (no Bluetooth / Wi-Fi Direct code).
- **Status:** `store-and-forward-mesh-relay` RESEARCH / working; offline payments not built.
- Possible difference: header-anchored verification so a phone can check a balance proof offline; depends on `light-client` (mobile readiness not verified).

## Legal and ethical gates

- Spectrum rules: EU 868 MHz sub-band duty-cycle limits (check ETSI EN 300 220); US 915 MHz under 47 CFR 15.247 (secondary source only). The code models EU868 airtime (`hal/radio-transport/src/frame.rs:38`).
- Radio encryption rules vary by country; amateur bands forbid it.
- Disaster use must never be presented as reliable (Bridgefy's history).
- No phone mining; visible opt-in only.

## What may be said publicly

- Accurate: "Maya2C includes a LoRa relay for block headers and proofs; it cannot carry transactions over that link."
- **forbidden**: "first offline payment network", "only mesh with post-quantum", "unprecedented", "works without internet", "disaster-proof payments".

**Verdict:** RESEARCH. Promotion would need a phone transport, a way to carry or defer 13 KB transactions, an independent review of the messaging layer, and a regional spectrum table.

