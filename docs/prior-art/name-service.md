# Prior art: Name service (name to address, renewal, anti-squatting, look-alike protection)

**Search status:** searched 2026-11-12 (web search; sources cited below). Solana Name Service returned no usable result in two queries; marked **not searched**.

Feature: parity row P-3, CATALOG row `names`, [ADR-105](../adr/ADR-105-name-service.md).

**Queries:** "ENS registration renewal premium auction homoglyph normalization ENSIP-15"; "ENS .eth registration temporary premium decay 21 days grace period 90 days renewal pricing; Solana Name Service SNS domains".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| ENS (Ethereum) | `.eth` names rented by the year; price by length; a 90-day grace period after expiry at the normal renewal price; then a 21-day temporary-premium auction starting at $100 million decaying exponentially to zero, so an expired name cannot be sniped at once. ENSv2 shortens the grace period. | [name lifecycle](https://support.ens.domains/en/articles/8046877-name-lifecycle), [temporary premium](https://support.ens.domains/core/registration/temporary-premium), [EP 6.43](https://docs.ens.domains/dao/proposals/6.43/) |
| ENSIP-15 normalization | Unicode normalization with rules against zero-width and confusable characters; names that fail were invalidated and refunded by DAO vote (117 ETH, proposal 4.3). Homoglyph names are still registrable within the allowed set; ENS support warns buyers. | [ENSIP-15](https://docs.ens.domains/ensip/15), [how normalization works](https://ens.domains/blog/post/how-ens-normalization-works), [homoglyphs](https://support.ens.domains/en/articles/7901658-homoglyphs), [proposal 4.3](https://docs.ens.domains/dao/proposals/4.3) |
| ENS commit-reveal | Registration is a commitment, a wait, then a reveal, so a block producer cannot front-run a name. | [ENS docs, name processing](https://docs.ens.domains/resolution/names) (registration flow referenced; the two-step detail is from the ENS registrar contract, general knowledge) |
| Solana Name Service | `.sol` names, one-time purchase. **Not searched** (no result returned). | — |

## What Maya2C has (checked in repo, 2026-11-12)

- Addresses have a Bech32m text form with network prefixes `maya`, `tmaya`, `dmaya` (ADR-081; `crates/address/src/lib.rs:18-70`), charset `qpzry9x8gf2tvdw0s3jn54khce6mua7l` (`crates/address/src/bech32m.rs:7`).
- A look-alike refusal already exists in the DEX front end for symbols (`crates/reference-apps/src/dex_front.rs:7`) and in `crates/design-system/src/specimen.rs`.
- No name records, no name transaction kind. CATALOG row `names` says "VM contract or a new state prefix".
- `payee-check` (CATALOG) is a wallet-side label check, not a registry.

## How the Maya2C version differs

- **ASCII-only labels by rule** (`a-z`, `0-9`, `-`), so homoglyph attacks across scripts are refused at the wire, not normalized. ENS admits most of Unicode and warns. The cost: no non-Latin names in v1, stated in the ADR.
- **Commit-reveal and a decaying post-expiry premium** are adopted from ENS; the premium schedule is in blocks, never wall-clock time.
- **Reverse records only from the address owner**, so a name cannot claim an address it does not control.
- A name record may carry a stealth meta-address (ADR-102), which ENS does through a separate registry (ERC-6538).

**Verdict: similar exists (ENS). Differentiate by the restricted charset and height-based lifecycle. No "first" claim.**
