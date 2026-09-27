# Report 30: Adoption, reference apps and public proof

Master Prompt 30. **The headline finding came from building the reference
apps: a Maya VM contract cannot learn who called it.** No contract that owns
anything can be secured until that changes. ADR-026 proposes the fix, and it
is reported rather than worked around.

| DONE WHEN | Status |
|---|---|
| Reference apps run on testnet | **Partly.** Four run end to end in process against the crates the node links. None runs on a testnet, because none is public. The fifth (NFT game) runs in the VM and is blocked by ADR-026 |
| Migration kits tested, with measured port times | **Partly.** Three kits are written (`docs/migration/`). One port was measured (81 s, by an agent), and no human port was |
| Grants and hackathon kits ready for approval | **Met**, as drafts: `docs/ecosystem/GRANTS.md` and `HACKATHON_KIT.md`, each marked as waiting for `APPROVED:` |
| Metrics dashboard live on testnet data | **Not met.** `cargo xtask eco-metrics` computes what the repository supports; there is no testnet to read |
| Public benchmark and beat-bar reports written, waiting for approval | **Met**: `docs/public/BENCHMARK_REPORT.md` and `BEAT_BAR_STATUS.md`. The benchmark is not independently reproduced, and the draft says so |
| `reports/30-adoption.md` complete | this file |

## 1. Reference applications: `crates/reference-apps`

```
$ cargo test -p maya-reference-apps --profile ci -- --nocapture
small: Quote { amount_in: 1000000, expected_out: 1992013, min_out: 1982052, impact_bps: 39, high_impact: false }
large: Quote { amount_in: 100000000, expected_out: 181322178, min_out: 180415567, impact_bps: 933, high_impact: true }
test dex_front_end_quotes_from_the_curve_refuses_look_alikes_and_catches_a_lying_page ... ok
test dao_a_passed_grant_pays_after_the_timelock_and_the_epoch_limit_binds ... ok
test vault_onboarding_flags_an_exposed_bitcoin_source ... ok
test payments_the_till_charges_only_the_merchant_within_scope ... ok
test vault_holds_a_large_withdrawal_guardians_cancel_theft_and_the_auditor_reads_only_memos ... ok
test result: ok. 5 passed; 0 failed
mint: 999 gas; module 2161 bytes
transfer: 981 gas
test anyone_can_move_anyones_token ... ok
test mint_transfer_and_level_up_follow_the_erc721_shape ... ok
test result: ok. 2 passed; 0 failed
```

The payments receipt read "Send 450 of token 0000…0000", until this report
fixed `clear-sign`'s rendering of the native coin.

| App | What its test proves | Showcases |
|---|---|---|
| Payments | A till session key pays only the merchant, at most 500 per payment, until its expiry height. The account's daily limit binds on top. Each refusal is the right error | MP22 smart accounts, clear-sign receipts |
| DEX front end | Quotes come from the curve (a 1% trade gets 1,992,013, not the spot 2,000,000). A 10% trade shows 933 bps impact and needs a second confirmation. `MАYA` (Cyrillic А) is refused as a look-alike. A page whose simulation adds an unlimited approval is caught (ClaimMismatch plus UnlimitedApproval) | MP29 guard, MP22 review, native AMM |
| DAO | A grant passes, waits out its timelock, executes, and pays. A second grant with 90% support is refused by the treasury's epoch limit. A grant without quorum is rejected | Governance, MP18 treasury |
| Quantum-safe vault | A 900,000 withdrawal is held, a guardian cancels it, and the vault is whole. The auditor's ML-KEM key opens the memo, and another key does not. A P2PK Bitcoin source is flagged as exposed | MP22 vault/guardians, MP27 viewing keys, MP28 exposure |
| NFT game | The ERC-721 shape works in the VM: mint, a once-only id, one level per block, and transfer, at about 1k gas. **Mallory moves Alice's token by claiming to be Alice** | the VM; the ADR-026 finding |

No stablecoin exists, so payments move the native coin. MP23's contract-safety
runtime and MP25's interop are not shown: the first is not wired into the VM,
and the second has no live light client to call.

## 2. The caller-identity gap: ADR-026 (Proposed)

- `HOST_FUNCTIONS` has no `caller`.
- `StateDB::call_contract` has the caller's address, uses it only to preload a
  balance, and passes the input through unchanged.
- The only prior contract, `token-swap`, has no per-user state, so the gap
  never surfaced. Nothing in the docs, the spec or `llms.txt` mentioned it
  before this report. It is now in all three places that matter:
  - `docs/audit/KNOWN_ISSUES.md` #15;
  - `llms.txt`;
  - each migration kit.
- The proposal is a `caller` host function at a written activation height.
- It is not implemented here. Changing the consensus-visible host surface is
  a decision somebody writes down, not a side effect of a template.

## 3. Migration kits: `docs/migration/`

- **Ethereum.** The brief's "Solidity via the EVM layer" path does not exist
  (ADR-023 keeps EVM out of v1), so the kit maps Solidity to Rust→WASM.
- **Solana** (Anchor) and **Move**: concept maps, what does not port, and a
  checklist for each.
- **Port time.** The NFT game took 81 s of wall clock from first line to
  passing VM tests (`2026-09-26T20:50:58Z` → `20:52:19Z`). The porter was an
  AI agent that had just read the token-swap contract's ABI. This is one
  data point; no human team has been measured. The four other apps are
  compositions of native modules, not ports.

## 4. Grants, hackathon, course: `docs/ecosystem/`

- **Grants.** Milestone-paid through `treasury` spends: two stages, an
  epoch limit, and the public ledger as the report. Budget, committee and
  veto window are left for the owner to set.
- **Hackathon.** Every starter command runs today.
  - **Finding:** the faucet's per-IP daily window gives a venue behind one NAT
    one grant a day. Either an allowlist or a pre-funded devnet is needed
    (KNOWN_ISSUES #16).
  - The contracts track should wait for ADR-026.
- **Course.** Nine lessons, each checked by an existing test. Not yet tested
  by beginners; the protocol is written. "Deployed" means a local devnet,
  since no testnet is public.

## 5. Ecosystem metrics: `cargo xtask eco-metrics`

- **Metrics computed:** active developers per month (agents counted
  separately), 30/90-day retention, and contract crates.
- **Named gaps:** deployed/used contracts, time to first deploy, SDK
  downloads, and grant outcomes.
- **Current result:** 1 human and 1 agent identity in 2026-09, and retention
  not yet measurable. The run is in a **shallow clone**, which the tool
  reports itself.
- The methods are in `docs/ecosystem/METRICS.md`, and the output carries no
  email addresses.

## 6. Public proof: `docs/public/`

- **Benchmark report.** All MP12–14/21 figures with their commands and the
  hardware record. It states what it does not show: no network TPS and no
  competitor comparison. **Not reproduced outside the project**, so not
  promotable.
- **Beat-bar status.** Nine bars: none met, and none regressed (there is no
  earlier release to compare with). Two changed since MP21 (blind signing,
  and developer time to a tested contract).
- **Announcement draft.** It makes no superlative claim: the one completed
  search (pq-from-genesis) forbids "first post-quantum L1".
- **`cargo xtask claims-check`** now enforces the prior-art standing order in
  CI. A superlative on a public surface must cite a prior-art record with a
  completed, dated search that does not forbid the phrase. Current run:
  31 public files, 0 unauthorised claims.
  - The detector is deliberately narrow: "first"/"only" before a product noun,
    plus "unprecedented", "world's first" and "first-ever". Ordinary
    technical English ("the first depositor") does not trip it.

## Not done

- A public testnet, and the reference apps deployed to it.
- A human-measured port, and beginner testing of the course.
- Independent reproduction of the benchmark.
- Every `APPROVED:` step. None was given, and nothing was published,
  announced or funded.

## Update 2026-09-27: the ownership blocker is gone

ADR-026 is accepted: the VM's `caller` host function reports the
transaction's signer, live from genesis. `contracts/nft-game` authorises
mint (admin), transfer and level-up (owner) by it, and now runs through the
node's own transaction path:

```
$ cargo test -p custom-l1-node --test nft_game_on_node_tests -- --nocapture
nft game on the node: deployed, minted to Alice; Mallory's signed theft was refused and changed nothing; Alice's transfer to Bob landed
$ cargo test -p maya-reference-apps --test nft_game
test only_the_signing_owner_moves_a_token ... ok
test mint_transfer_and_level_up_follow_the_erc721_shape ... ok
```

Still open for this prompt's DONE WHEN: a public testnet to run the
reference apps on (a local DAG-BFT devnet exists, `scripts/bft_devnet.py`),
measured port times with outside developers, a live metrics dashboard on
testnet data, and the owner's approvals of the grants, hackathon and
public-report drafts.
