# Prior art: Clear signing (intent standard + mandatory simulation)

**Search status:** partially searched 2026-09-26.

**Who else does something similar.** Bybit's loss came from blind signing ([NCC Group](https://www.nccgroup.com/research/in-depth-technical-analysis-of-the-bybit-hack/)). From knowledge, **not searched**: Ledger's clear-signing initiatives, ERC-7730, and wallet transaction simulation (e.g. Blockaid).

**How Maya2C differs.** `crates/clear-sign` (2026-09-26): an intent standard and a pre-sign review that trusts only the simulation; 220 synthesized drainer cases flagged. Wallet integration and simulation are not built, so nothing is user-facing.

**What may be said publicly.** nothing may be claimed.
