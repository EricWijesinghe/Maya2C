# Prior art: Post-quantum name service with commit-reveal registration

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: v4 P-3, [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-3, design text ADR-105 (PR #465), which has its own [name-service.md](name-service.md) on PR #465 for ENS/Handshake/SNS.

**Queries:** "quantum-resistant blockchain name service ENS alternative post-quantum signatures commit-reveal".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Quantova Name Service (QNS) | `.qtov` names on the Quantova chain; registrations and record changes authorised with Dilithium, Falcon or SPHINCS+; sub-domains registered through a commit-reveal flow against front-running. Project README; claims unverified. | [github mirror](https://gh.r-mj.com/Quantova/quantova-name-service) |
| ENS, Handshake, others | Names secured by secp256k1 keys; PQ checks could run in contracts at gas cost. Commit-reveal registration is ENS's standard flow. | [dn.org on PQ and name security](https://dn.org/post-quantum-cryptography-and-name-security/) |
| Hash-based commit-reveal research | A May 2026 preprint proposes two lightweight hash-carrying transactions as a quantum-resistant authorisation at ~1.5-2× footprint; bitcoindev reviewers note the anchor must commit to the reveal or a quantum adversary can replace it. | [arXiv 2605.06853](https://arxiv.org/pdf/2605.06853), [bitcoindev thread](https://groups.google.com/g/bitcoindev/c/rgjCuOVndds) |

## What Maya2C has (checked in repo, 2026-11-12)

- Bech32m address text (ADR-081), no name record, no name transaction kind.
- Every transaction signature is a registry PQ suite; the commit-reveal in
  ADR-135 §P-3 is against **front-running**, not a substitute for a PQ
  signature (the reveal is itself PQ-signed, which answers the bitcoindev
  objection above: the reveal cannot be replaced by a stranger because it
  is signed, and it cannot be front-run because the commitment is aged and
  bound to the sender).

## How the Maya2C version differs

- Names are native state records (`nm:`), resolved by wallets without
  running contract code; the reverse record is written by the address
  owner and must point back (`target == sender`).
- ASCII-only labels by rule, so no cross-script homoglyph exists in state;
  in-script confusables are a wallet warning.
- Rent by block count, not by wall clock (Standing Order 4).

**Verdict: similar exists (QNS does PQ-signed names with commit-reveal).
Differentiate by native records, the reverse-record rule and ASCII-only
labels. No "first" claim.**
