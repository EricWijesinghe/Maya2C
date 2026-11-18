# Prior-art notes: Protection for people (CATALOG section K)

Research behind [CATALOG section K](../ecosystem/CATALOG.md), searched
2026-11-10 from search results only. Nothing here is a novelty claim: a row
needs its own dated `docs/prior-art/` record before any "others don't"
wording (Standing Order 10). Links are recorded as found, not re-fetched.

## Prior-art notes (searched 2026-11-10, English)
- Need evidence: address poisoning 6,633 confirmed incidents, ≥ $83.8M lost, 270M poisoning transfers (Tsuchiya et al., USENIX Security 2025: https://www.usenix.org/conference/usenixsecurity25/presentation/tsuchiya, https://arxiv.org/abs/2501.16681v1). Bybit 2025-02-21 (~$1.4–1.5B) via injected JS changing what signers saw (https://www.nccgroup.com/research/in-depth-technical-analysis-of-the-bybit-hack/). Drainers $83.85M in 2025 (ScamSniffer, vendor, EVM only: https://drops.scamsniffer.io/scam-sniffer-2025-crypto-phishing-losses-fall-83-to-84-million/).
- undo-send: ERC-20R (https://arxiv.org/abs/2208.00543v2, never deployed); Circle refund-style counter-payments (secondary: https://decrypt.co/341374/circle-mulls-reversible-usdc-stablecoin-transactions-in-push-for-tradfi-adoption); BIP-345 OP_VAULT (https://www.github.com/bitcoin/bips/blob/master/bip-0345.mediawiki, closed). Difference: no judges; only sender or their guardians cancel, via verified consensus escrow.
- payee-check: Pay.UK CoP (https://www.wearepay.uk/confirmation-of-payee-reaches-two-billion-checks-helping-protect-uk-consumers-against-fraud/, weak evidence); EU VoP (https://legal.pwc.de/en/news/articles/verification-of-payee-requirements-vop-under-the-eus-instant-payments-regulation-ipr); ENS label spoofing (https://ar5iv.labs.arxiv.org/html/2104.05185).
- sim-firewall: Rabby simulation (https://support.rabby.io/en/articles/14124199-understanding-rabby-s-transaction-simulation), Blockaid for MetaMask/Safe; clear-sign has 220 synthesized cases, no real-world catch rate.
- poison-shield: Etherscan mitigations (https://info.etherscan.com/address-poisoning-attacks-are-rising-on-ethereum). clear-sign uses `mul_wide` (512-bit, `lib.rs:217`) — off consensus, under Eric's open "no 512" decision.
- second-screen-check: Candide guardians (https://docs.candide.dev/wallet/plugins/recovery-with-guardians), Liana (https://docs.rs/crate/liana/3.0.0).
- panic-sweep: OP_VAULT_RECOVER (BIP-345); Harpie shutdown (https://cryptorank.io/news/feed/5eb64-harpie-web3-security-tool-announce-shutdown).
- permission-center: Coinbase Spend Permissions (https://docs.cdp.coinbase.com/wallets/using-wallets/spend-permissions); ERC-7715 draft.
- cooling-off: UK PSR APP reimbursement from 2024-10-07, £85k cap (https://www.finextra.com/pressarticle/102475/psr-confirms-85k-reimbursement-limit-for-app-fraud-victims); no evidence found that new-payee delays reduce pig-butchering losses.
- scam-registry: Chainabuse/TRM (https://docs.trmlabs.com/guides/chainabuse/verifying-the-accuracy-of-reported-information); off chain because a `t:` record under the state root is permanent.
- intent-descriptors: ERC-7730 (https://ethereum.org/developers/tutorials/clear-signing).
- mutual-cover: Nexus Mutual is discretionary (https://docs.nexusmutual.io/resources/faq).
