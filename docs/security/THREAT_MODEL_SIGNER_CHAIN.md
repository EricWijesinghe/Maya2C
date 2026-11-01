# Signer-chain threat model: Lazarus-class attacks

Status: written 2026-10-31 from code and config as they stand on
`docs/threat-model-signer-chain` (off `master`). Every control named below was
read in the source or workflow file cited. Nothing here was run: no test
output, no build, no live check is claimed. Where a control rests on an owner
action outside the repository, it is marked **owner-attested**. Every external
review is marked **external test pending**. No external pentest or audit has
been commissioned or performed.

## 1. The attack class

In February 2025 (Bybit; loss figure and attribution from public reporting, not re-verified here), the attacker compromised a developer machine at the Safe{Wallet}
supplier, changed the JavaScript that rendered the signing UI, and the
signers approved a transaction whose screen showed a normal transfer while the
calldata moved the cold wallet's control to the attacker. The signing device
signed what it was shown. Nothing checked the screen against the bytes.
Reference: NCC Group analysis linked from `docs/prior-art/clear-signing.md`.

The class has four moving parts. Any one of them, broken, moves funds:

| Step | Lazarus pattern | Maya2C surface |
|---|---|---|
| S1 | Compromise a dev or build machine, or a vendor's CI | Eric's PC (builds, keys, validators), GitHub Actions, npm |
| S2 | Ship a different UI or script to the signer | wallet-gui WASM bundle, Hub, maya2c.dev (Cloudflare edge), explorer |
| S3 | The signer approves a screen, not the bytes | every signing path: wallet-gui, l1-wallet, Ledger (not shipped), Hub miner payout |
| S4 | Value leaves through a path the screen did not name | fee collector, genesis/chain id, vault limits, miner binary, updater |

Our own incident, 2026-10-06, is the same shape at a smaller scale. A stolen
Cloudflare API token attached a script to every page of maya2c.dev (explorer,
status and faucet included). The script showed a fake "verify you are human"
box that asked visitors to paste a terminal command. The GitHub Pages origin
stayed clean. Evidence in `D:/Maya2C-incident/` (outside the repo):

- `notice-draft.md`: draft notice, status DRAFT, names the Cloudflare token and a Worker.
- `alerts.log` line 84: `maya2c.dev/ differs from origin by 5558 bytes` on 2026-10-13.
  The incident folder we read does not record a closing explanation for that diff.
- `alerts.log`: repeated `origin unreachable` entries from 2026-10-16 to 2026-10-31,
  so the watch could not compare those runs.
- `integrity.log`: the last entries (2026-10-31) read `clean`.
- `integrity-watch.ps1`: hourly; runs on Eric's PC; compares each page as Cloudflare
  serves it against the origin pinned by IP.

The repository's response to that incident is a Content-Security-Policy with hashes (`docs/site/astro.config.mjs:27`) and
`docs/site/scripts/check-csp.mjs` (checks that a page's hashes match its CSP).

## 2. Trust roots

A forged screen, release or dependency needs one of these to be compromised.
Listing them is the starting control: each is a place to put a second check.

| Root | Who controls it | Protects |
|---|---|---|
| Eric's PC | one person, one machine | validator keys (`D:\Keys`, ADR-049), local builds, the owner's `gh` session, the incident watch |
| GitHub repo + Actions | one maintainer account (`.github/CODEOWNERS:3`: `* @EricWijesinghe`) | source, release tags, `TAURI_SIGNING_PRIVATE_KEY` and `_PASSWORD` secrets |
| Cloudflare account | one account; token scope not in repo | maya2c.dev edge, `rpc.maya2c.dev` tunnel, status/explorer/faucet subdomains |
| npm registry + `docs/site/package-lock.json` | registry, lockfile hashes | site build, SDK publish (`publish_sdk.yml`, with provenance) |
| crates.io + `Cargo.lock` | registry, lockfile | every binary; `deny.toml`, `supply-chain/` |
| Update signing key (not yet live) | same GitHub secret | in-app updates for wallet, chat, hub |
| Chain genesis id | compiled into wallet builds for mainnet (currently empty) | which chain a signature is for |

Repo-side protections, as written in `CONTRIBUTING.md:28-34`: no direct pushes
to `master`; required checks reality ledger, spec conformance, rustfmt, clippy,
nextest (core), cargo-deny; no force-push; release tags creatable by the maintainer and no one else. The GitHub settings that enforce those rules are **owner-attested**;
the repo does not contain a ruleset export.

## 3. Paths

Each path: the attack, the existing control with evidence, the gap, the control
to add, and the test that would prove the control. Ranks are in section 4.

### P1. Forged signing screen in wallet-gui (the Bybit shape)

- **Attack.** The WASM UI (`apps/wallet-gui/ui`) is replaced, or its logic is
  altered, so that the screen shows the intended transfer and the call that
  signs sends a different one. Entry points: a malicious release (see P6),
  a modified local install, or a supply-chain hit on the UI crate (P8).
- **Existing control.**
  - Tauri CSP `script-src 'self' 'wasm-unsafe-eval'`, `connect-src 'self' ipc:`
    (`apps/wallet-gui/src-tauri/tauri.conf.json`, `app.security.csp`). Stops injected
    third-party script; does not stop a replaced bundle served from `self`.
  - Preview then sign, two steps in the UI (`apps/wallet-gui/ui/src/screens.rs:740-770`).
  - The signed values come from the preview summary, not from the form.
- **Gap.**
  - `preview_transfer` (`apps/wallet-gui/src-tauri/src/commands.rs:274`) echoes its
    inputs and nothing else. It is not a binding: nothing in the core records what was previewed.
  - `sign_transfer` (`commands.rs:315`) accepts `recipient`, `amount`, `fee`, `fee_to`,
    `nonce` and `genesis` as plain arguments from the webview. A UI that
    skips the preview, or edits the summary, signs whatever it passes.
  - The core never renders the transaction itself, so the words on screen come
    from the same process that can be replaced.
- **Recommended control.** The core keeps a preview record: on preview it computes
  a digest over the canonical transfer fields (sender, recipient, amount, fee,
  fee_to, nonce, genesis) and returns an opaque token bound to that digest and
  to the session. `sign_transfer` takes the token and nothing else, re-derives the fields
  from the token's record, and refuses any other input. The core also returns
  the human summary text it generated, so the screen can show core-generated
  text. Expiry: a short TTL, single use.
- **Test.** `wallet-gui` core: `sign_refuses_when_fields_differ_from_preview`
  (change recipient, amount, fee_to, nonce or genesis after preview; expect
  refusal); `preview_token_is_single_use`; `preview_token_expires`. Current
  state: **no such tests exist** (these are the tests to write).
- **Residual.** A malicious build that also edits the core is out of reach of
  any in-process check. That is P6/P16's problem: the release must be
  reproducible and signed, and the owner must verify the hash before installing.

### P2. Forged web page (maya2c.dev, explorer, faucet, status, portal) via the CDN

- **Attack.** The edge (Cloudflare Worker, Snippet, Zaraz, or a stolen token that
  can add one) injects or replaces script on any page. The page then shows a
  fake "verify", a fake "connect wallet", or asks to paste a command. This is
  the 2026-10-06 incident.
- **Existing control.**
  - CSP with hashes, built by `docs/site/astro.config.mjs` and checked by
    `docs/site/scripts/check-csp.mjs`. An injected script with no hash in the CSP does not run.
  - Hourly integrity watch (`D:/Maya2C-incident/integrity-watch.ps1`): compares
    each page served through Cloudflare with the GitHub Pages origin, byte for
    byte, and checks for the attacker's markers and the `/_r/` route.
  - Pages origin deploy from a build job (`.github/workflows/site.yml`).
- **Gap.**
  - The CDN account is outside the repo. The repo cannot enforce its rules.
    Owner rotated tokens and enabled 2FA (`notice-draft.md`, "What we did"):
    **owner-attested**, not verifiable from the repo.
  - The integrity watch runs on Eric's PC. If the PC is down, the watch is down.
    The watch's log shows `origin unreachable` repeatedly from 2026-10-16. It
    is blind for those runs.
  - The 5558-byte diff on 2026-10-13 (`alerts.log:84`) has no closing entry in
    the incident folder that was read. It must be explained or ruled out in writing.
  - The integrity watch is a GET-based comparison. It does not check
    subresource integrity on links, or the `/_r/` path once a route changes.
  - A stolen CDN token could also change DNS, the tunnel or the update manifest;
    none of those is in the watch.
  - No CI job compares the built `dist/` to what the edge serves.
- **Recommended control.** (a) Move the watch to a second machine or to a
  scheduled CI job that does not share the PC's trust. (b) Add SRI for every
  script and stylesheet the build emits. (c) Add a CI job: built output hash
  equals the live response hash, per page, per deploy. (d) Scope the CDN token
  to the minimum (zone, no account admin). (e) Publish the integrity watch's
  rules (what it checks, what counts as an alarm) so a reader can tell
  "clean" from "not checked".
- **Test.** `check-csp` with an injected inline script: expect failure
  (exists for hashes; add the injection case). New: `site-integrity` CI job
  fails when a fixture response differs from the built file. New: SRI check
  fails a build when a script tag lacks an `integrity` attribute.

### P3. Malicious or unpinned RPC endpoint steers a signature (wallet-gui)

- **Attack.** The node or `rpc.maya2c.dev` answers `get_chain_info` (genesis id),
  `get_fee_info` (collector, base fee) and the nonce. The wallet signs for what
  it was told.
- **Existing control.**
  - Fee collector pinned in the wallet: `check_fee_collector`
    (`apps/wallet-gui/core/src/payment.rs:357`) compares to
    `custom_l1_node::state::fees::FEE_COLLECTOR` (`crates/node/src/state/fees.rs:34`).
    Called from `fee_quote` (`apps/wallet-gui/core/src/wallet.rs`).
  - Network classification: `apps/wallet-gui/core/src/networks.rs` pins mainnet
    genesis at build time; a node that claims a mainnet name without the pinned
    genesis is `Impostor` and refused (`apps/wallet-gui/src-tauri/src/commands.rs:476`).
  - Preview names the recipient from the user's input, not from the node.
- **Gap.**
  - `MAINNET_GENESIS` is empty (`networks.rs:14`) until the genesis ceremony. Until then the
    mainnet check cannot refuse a real-looking mainnet genesis.
  - Testnet genesis is not pinned in any build. A node that reports a genesis
    id the attacker controls gets a valid signature for that chain. Replay
    onto a state copy under the attacker's control is the risk; the user's
    intended chain loses the transfer.
  - Fee collector check covers `fee_quote` alone; the genesis used to sign comes from
    `chain_info` without a pin.
- **Recommended control.** Pin the testnet genesis in the wallet build (a list
  like `MAINNET_GENESIS`, one entry per network). Refuse signing when the
  node's genesis is outside the pinned list for its class. Show the genesis
  fingerprint in the confirm screen (P1's core summary).
- **Test.** `classify` table test already exists for mainnet (`networks.rs` tests).
  Add: `testnet_genesis_not_pinned_refused_for_signing`;
  `sign_terms_refuse_unpinned_genesis`.

### P4. l1-wallet CLI signs without a confirmation and pays a node-chosen collector

- **Attack.** A hostile node, or a hostile RPC, quotes a fee and collector. The
  CLI pays it. The description is printed to the terminal, which the attacker
  does not control, but the CLI does not stop for a reply.
- **Existing control.**
  - `print_description` prints the transaction before signing
    (`bins/l1-wallet/src/main.rs:432-441`, `:469`), from the shared
    `crates/node/src/core/describe.rs` describe function (`Review::Careful` at `describe.rs:30`).
  - The auto-fee is refused above `u64::MAX` base units (`main.rs:~538-541`):
    the fee must then be chosen with `--fee`.
- **Gap.**
  - No interactive confirmation. Print, then sign (`main.rs:469-473`).
  - The collector is taken from `get_fee_info` without the pin used by the GUI
    (`main.rs:513`; compare `payment.rs:357`).
  - The auto-fee cap is `u64::MAX` base units. That is not a small amount on
    small balances, and it is a number chosen in code, not measured.
- **Recommended control.** Require `y` on the terminal, or `--yes` in scripts
  with a logged line. Pin the collector with the same `FEE_COLLECTOR` check.
  Cap the auto-fee to a fraction of the amount and fail above it.
- **Test.** `l1_wallet_refuses_unpinned_collector`;
  `l1_wallet_send_requires_confirmation_without_yes` (drive stdin, expect no broadcast).

### P5. Forged or swapped miner binary, or key file, from Hub settings

- **Attack.** The Hub spawns the miner binary named in its settings, and passes
  the payout key file path. A changed setting, or a changed UI, runs an
  attacker binary, or points the payout at the attacker's key.
- **Existing control.**
  - Hub spawns the miner next to its own executable where the bundle puts it
    (`apps/hub/src-tauri/src/lib.rs:36`, `miner_beside_hub`), and checks it is a file.
  - `check_url` on the RPC (`apps/hub/src-tauri/src/miner.rs:144`).
  - "The hub never hashes or signs: the miner binary owns the payout key file" (`miner.rs:4`).
- **Gap.**
  - `config.binary` is read from settings (`miner.rs:139-146`) and not checked
    against the bundle path or a hash. Any settings edit redirects execution.
  - Key file path is not checked for ownership or mode.
  - The Hub's updater (`lib.rs:107-108`) is wired, so a bad update also reaches this path (P7).
- **Recommended control.** Accept the bundled miner, or a binary whose
  SHA-256 matches a value in the signed release manifest. Refuse a key file that
  is not owned by the current user, and warn when its mode is open.
- **Test.** `hub_refuses_miner_outside_bundle`; `hub_refuses_key_file_not_owned_by_user`
  (Windows ACL check).

### P6. Forged release binary (release pipeline)

- **Attack.** A tag, a workflow edit, a runner compromise, or a stolen maintainer
  session publishes a binary that differs from the source.
- **Existing control.**
  - Release tags: maintainer-gated, immutable (`CONTRIBUTING.md:31-33`).
  - Least-privilege workflow token: `contents: read` at the top (`.github/workflows/release-binaries.yml:27-28`).
  - SHA-256 of every file, plus an SBOM (`release-binaries.yml:398-409`).
  - SLSA build provenance attested over `dist/SHA256SUMS`, which covers every file (`release-binaries.yml:412-416`). Verify with `gh attestation verify`.
  - Linux node built reproducibly (ADR-054; `.github/workflows/reproducible-build.yml`).
- **Gap.**
  - Windows and macOS builds are not reproducibility-checked (`release-binaries.yml:67`).
  - `reproducible-build.yml` is not a required check (its header says so; `CONTRIBUTING.md` lists required checks without it).
  - Installers are not OS-code-signed. The release notes say so
    (`release-binaries.yml:426`): Windows and macOS warn about an unknown publisher.
    That warning trains users to click through, which is the social-engineering opening (P15).
  - Provenance proves where a file was built, not that the build was reviewed.
    A signed build of a malicious commit is still signed.
  - One maintainer can merge (`.github/CODEOWNERS:3`). The "two human reviewers"
    standing order is a note, not enforced (`CLAUDE.md`, production standing orders).
- **Recommended control.** Require the reproducible-build check on release tags.
  OS code signing for installers (Authenticode, Apple notarization) with the
  signing keys held off the build runner. A second human approval for any change
  to `.github/workflows/`, `release-binaries.yml`, or `apps/*/src-tauri/tauri.conf.json`
  (via CODEOWNERS path rules when a second reviewer exists). Publish the
  verification command next to every download.
- **Test.** A release-job step that runs `gh attestation verify` on each asset
  and fails the job if any fails; a CI check that the required-checks list
  includes `reproducible-build` for tags; a workflow lint that fails if a
  `permissions:` block is widened.

### P7. Update channel (in-app updates, wallet, chat, hub)

- **Attack.** The update manifest or bundle is replaced (CDN, P2), or the update
  key is used to ship a bad build to every installed app.
- **Existing control.** Updates need a click ("Install now", `docs/in-app-updates.md`).
  The bundle is checked against the built-in public key (`apps/app-update`).
  Manifests are generated by `docs/site/scripts/gen-updates.mjs`.
- **Gap.**
  - The updater is **inert today**: `apps/updater-pubkey.txt` does not exist
    (checked), so no app offers an update. The doc says so ("Until both steps
    are done ... the apps never offer an update").
  - When it is turned on: the signing key is a GitHub secret, one account can
    read it, and a single maintainer can start the release that uses it.
  - Rollback protection (refuse an older signed version) was not checked in this pass; treat as **not verified**.
  - The key-loss path is documented (reinstall by hand). A key-compromise path
    is not documented.
- **Recommended control.** Before enabling: move the update key off GitHub
  secrets to an offline signer with a second person present; add monotonic
  version refusal in `apps/app-update`; document key rotation and the
  revocation path; keep the manifest on a path that P2 does not share.
- **Test.** `updater_refuses_older_version`; `updater_refuses_bundle_not_signed_by_pinned_key`;
  `updater_refuses_manifest_pointing_off_origin`.

### P8. Supply chain: Rust crates, npm packages, CI actions

- **Attack.** A dependency update, a typosquat, or a hijacked maintainer ships
  code into a binary, a UI bundle, or the site build.
- **Existing control.**
  - `cargo deny` is blocking in CI (`security-audit.yml:335-339`) and in the
    required list (`CONTRIBUTING.md:28-30`). Unknown registries and git sources
    are denied (`deny.toml:104-105`). Advisory exceptions are in `deny.toml`
    with reasons.
  - `Cargo.lock` committed; dependabot configured (`.github/dependabot.yml`).
  - Actions pinned to commit SHAs throughout the workflows read.
  - npm: `npm ci` against `docs/site/package-lock.json` (`site.yml:46`), and
    `publish_sdk.yml` publishes with `--provenance` (`:118`).
- **Gap.**
  - `cargo vet` runs in report mode, with 1068 registry crates exempted in
    `supply-chain/config.toml` (count from `rg`). It does not block.
  - Site build job: `site.yml` sets `id-token: write` and `pages: write` at the
    workflow level (`:24-25`), and the build job does not override them. `npm ci`
    runs lifecycle scripts by default. A malicious package in the site build
    can therefore reach the job that writes the Pages artifact, which is the
    origin (not the CDN). The origin was clean in 2026-10; the path is still open.
  - `ci.yml:146`: `npm install --no-save --prefix design/visual playwright@1.56.1`
    runs without a lockfile. Version is pinned; integrity is not.
  - No `--ignore-scripts` on any `npm ci`.
- **Recommended control.** Give the site build job `id-token: none` and
  `pages: read`; split the deploy job out; run `npm ci --ignore-scripts` in the
  build, and allow the few packages that need scripts by name. Lock the
  playwright install with a lockfile. Make `cargo vet` blocking for the core
  crates (crypto, state, consensus) before mainnet; leave the rest in report mode.
- **Test.** A workflow lint (zizmor or an xtask check, to be chosen) that fails on
  `id-token: write` in a build job; `npm ci --ignore-scripts` with a named
  allow-list; `cargo vet check --locked` on the core crate set, required.

### P9. Validator consensus keys at rest (ADR-049)

- **Attack.** Local malware or another local account reads the consensus key
  and signs as the validator (equivocation risk on a live chain).
- **Existing control.**
  - Sealed format: Argon2id 64 MiB, ChaCha20-Poly1305, public key and KDF
    parameters bound as associated data (`docs/adr/ADR-049-sealed-validator-keys.md`, Decision 1).
  - Passphrase from a file, never argv or env (Decision 2).
  - A `production` build refuses plaintext keys (Decision 4).
  - Atomic writes with verify-before-link (Decision 3).
- **Gap.**
  - Testnet and devnet builds still load plaintext keys with a warning (Decision 4).
  - On Windows there is no 0600; the key inherits its directory's ACL (Decision 6).
    The ACL lockdown is recorded outside the repo (2026-10-17 audit); **owner-attested**.
  - BitLocker was off on the audited volumes (ADR-049 Context). Not re-verified.
  - Mainnet is planned from Eric's PC (the PC launch decision): a full-disk
    encryption gap plus a single host means P16 and P9 are the same risk.
  - Zeroization is required by Standing Order 5 and Execution Directive 1;
    not traced for every path in this pass.
- **Recommended control.** Require the sealed format on every network (not just production) before any run that holds value. Turn on full-disk encryption and
  verify it. Keep the passphrase on a separate unlock path. Move the consensus
  key to an HSM or a separate signer host before mainnet (ADR-049 follow-up).
- **Test.** `validator_refuses_plaintext_on_mainnet_config` (config-level, not
  build-level); `key_dir_acl_open_refused` (Windows; ACL read in test); existing
  seal/migrate tests in `crates/signer` should be checked against Decision 3.

### P10. custody-mpc: the combiner holds the key during one signature (invariant 18)

- **Attack.** Whoever controls the combiner machine during a signing round
  signs anything the vault owns (`docs/invariants.md:92`, invariant 18).
- **Existing control.**
  - Five custodians, no dealer, shares split so any two learn nothing (`docs/custody-mpc.md`).
  - Every reconstruction is checked against the vault commitment before a key is
    derived (`vss::check_opening`; invariant 19, `docs/invariants.md:103`).
  - The design choice is stated in the code and the docs, and the invariant forbids
    quiet relaxation.
- **Gap.**
  - Combiner isolation is not specified: no hardware boundary, no attestation
    of the combiner process. This is the Bybit shape at custodian scale: the
    combiner sees the message and the key.
  - The message shown to custodians is not verified against the signed bytes
    (P1's preview-binding gap, at the custodian level). **Not verified** in the
    custody crate read; treat as open until a test shows otherwise.
- **Recommended control.** Custodians sign a rendered summary produced by the
  combiner's own rules, and the combiner refuses any request whose summary
  differs from the bytes. Run the combiner on an isolated host or TEE before any
  vault holds value. Record this as a named residual in the ADR.
- **Test.** `combiner_refuses_message_not_matching_approved_summary`;
  `combiner_refuses_short_quorum` (exists in spirit via invariant 19; confirm the test).

### P11. Multisig and vault accounts (invariant 32)

- **Attack.** A guardian or signer key is compromised and the attacker moves a
  vault's balance beyond its window limit, or cancels the owner's withdrawal
  request, or sends everything to the fee collector.
- **Existing control.**
  - Chain-enforced: a vault moves at most its `limit` per `delay_blocks` window
    without a delayed, guardian-cancellable request (`docs/invariants.md:230`,
    invariant 32). The fee collector counts toward the limit above 4x the fee.
    Pinned by `crates/node/tests/vault_tests.rs`, ADR-030.
  - `crates/smart-account` implements multisig and guardian logic; not traced in this pass.
- **Gap.**
  - The wallets do not expose vault actions with a review of the delay and the
    limit (`apps/wallet-gui`, `bins/l1-wallet` have no vault screen in this pass).
    A user cannot see the cancel window in the signing step.
  - Guardian keys: storage and rotation are not documented in the code read.
  - Guardian cancellation is a second chance to stop a theft if the guardian
    is awake and on a clean machine.
- **Recommended control.** Vault actions show the delay, the limit, and the
  cancel path in the core summary (P1). Document guardian key custody. Alert
  guardians on every request, from a channel that P2 cannot forge.
- **Test.** Existing `vault_tests.rs` for the chain rule; add
  `vault_summary_states_delay_and_limit` once a summary exists.

### P12. Governance capture and the emergency pause

- **Attack.** A quorum of stake or a committee passes a parameter change that
  reduces safety, or the council pause is used to halt funds, or the council is
  social-engineered into a pause that a rogue group relies on.
- **Existing control.**
  - Floors compiled into the binary, not governable: quorum 1000 bps, approval 5001 bps,
    voting 480 blocks, timelock 960 blocks (`crates/governance/src/limits.rs:33-71`, invariant 12).
  - No program in chain state; a rule change moves a number or chooses a shipped implementation (invariant 13).
  - Council pause exists and is tested: `crates/node/tests/council_pause_tests.rs`;
    the pause never transfers and expires (`docs/security/INCIDENT_RESPONSE.md`).
  - Committee floor (ADR-070).
- **Gap.**
  - Committee capture and quorum at small validator counts is gate 10 in the
    mainnet plan. Not closed in the repo as of this read.
  - The pause's signer set and its custody are not in the repo. **Owner-attested** at most.
  - Social engineering of the council (P15) is not modelled.
- **Recommended control.** Publish the council's signer set and key custody.
  Require two-of-n out-of-band confirmation (a call, not a message) for the pause.
  Close gate 10 with a written decision before mainnet.
- **Test.** `council_pause_tests.rs` exists; add a test that the pause refuses a
  request lacking the second confirmation, once that exists.

### P13. Ledger application (not shipped)

- **Attack.** Device shows one thing, the host sends another.
- **Existing control.** `apps/ledger-maya2c/src/review.rs:13-17`: payload kinds the
  device cannot describe are refused rather than shown as a transfer. The
  device shows the address and every output. Feasibility, not product: suite
  `0x10` alone; the hybrid does not fit the device RAM (`docs/ledger-feasibility.md`).
- **Gap.** No shipped device path, so this is not a live risk today. If shipped
  as a transfer-type signer, the device's screen is the sole trust anchor and
  the host's copy of the transaction must be checked against it byte for byte.
- **Recommended control.** Do not ship a device signing path before the screen
  review is tested against a fuzzed host; state in the release notes that the
  device is not a mainnet signer until then.
- **Test.** `review.rs` refusal tests exist (`apps/ledger-maya2c/tests/`); add a
  host-vs-device mismatch fixture.

### P14. Explorer as a trust source

- **Attack.** The explorer's plain-English summary for a transaction is shown
  as truth; the indexer or the CDN changes it.
- **Existing control.** The summary is derived from the same `core::describe`
  rules (`apps/explorer/src/plain.rs:34`, `crates/node/src/core/describe.rs:105`).
  The node's RPC returns a `careful` flag, computed by the node (`crates/node/src/rpc/types.rs:330-377`).
- **Gap.** The explorer's view is served through the CDN (P2). A user who checks
  a transfer on the explorer checks a page the attacker can change. The explorer
  does not verify the summary against the transaction bytes.
- **Recommended control.** Show the txid and the raw bytes hash, so the
  reader can compare with the wallet. Mark the explorer as a viewer, not a verifier, in its own UI.
- **Test.** A fixture where `plain.rs` and the core describe disagree must fail CI.

### P15. Social engineering (paste-a-command, fake update, fake support)

- **Attack.** A visitor is asked to paste a command, install an "update", or send
  a seed phrase. This is how the 2026-10-06 incident was aimed at users.
- **Existing control.** `notice-draft.md` tells users what to do and that Maya2C
  never asks them to paste a command (draft; not yet published). CSP on the site
  (P2). Installers have SHA-256 and provenance (P6).
- **Gap.** Installers trigger OS warnings (P6), which trains click-through. The
  notice is a draft. No documented out-of-band verification for a release
  (a second channel that P2 cannot touch).
- **Recommended control.** Publish the release hashes in a signed text file
  (PGP or age) with a key fingerprint printed in this repo's README and in the
  owner's own profile. Add an "if asked for a seed phrase or a command, stop" line
  to every wallet screen (core-generated text, P1).
- **Test.** A screen-text test that the warning string is present in the wallet
  build; the notice is published after the sign-off in its header.

### P16. Dev machine or CI as the single point of compromise (the Bybit origin, applied to us)

- **Attack.** Eric's PC is compromised. It holds validator keys, the `gh` session,
  the incident watch, the GitHub secrets' source file, and the local build. A
  build from it, or a signed release, carries the attacker's change.
- **Existing control.** Keys in sealed format (P9); ACL lockdown (**owner-attested**);
  release built in CI (`release-binaries.yml`); tags maintainer-gated.
- **Gap.** One machine, one account, no second signer, no hardware key in the repo's
  documented flow. The owner's local `D:\Keys` directory is the same host as the
  builds the owner reviews. The PC is also the watch. Nothing in the repo
  defends against that host.
- **Recommended control.** Hardware security key for GitHub and Cloudflare (**owner-attested**
  whether done). Release signing off the PC. A second person with a verified
  key for any release signature. Separate the watch host from the build host.
- **Test.** Not a code test; a drill: rotate every credential in a tabletop, and
  check the drill's output into `reports/`. **External test pending** for an independent review.

## 4. Ranked gap table

Ranked by: can it move funds or ship code to users, and how close is it to
being live. CRITICAL blocks mainnet. HIGH must close before mainnet or before
the path is enabled. MEDIUM is a judgment call with a written reason.

| Rank | Gap | Path | Why this rank | Control to add | Test that proves it |
|---|---|---|---|---|---|
| CRITICAL | Sign args from the webview; preview is an echo, no core binding | P1 | Bybit shape. Changing one argument changes the signature | Core preview token bound to canonical fields; `sign` takes the token alone | `sign_refuses_when_fields_differ_from_preview` |
| CRITICAL | No clear-sign review or simulation on any user signing path (`maya-clear-sign` is used by `bins/maya2c-mcp` and `reference-apps`) | P1, P4, P11 | Blind-signing is the cause of the 2026 loss; the review exists but is not on the path | Call `review(claimed, simulated, ctx)` before every signature; refuse on `ClaimMismatch` or drainer patterns | `wallet_refuses_sign_on_claim_mismatch` |
| CRITICAL | Web and CDN are outside repo controls; integrity watch runs on the owner PC and was blind for runs from 2026-10-16; 5558-byte diff unexplained in the read record | P2, P15 | Already exploited once. The affected surface is every page | Second-host watch; SRI; CI build-vs-live hash check; token scoping (owner) | `site_integrity_job_fails_on_fixture_diff`; SRI lint |
| CRITICAL | Single host and single account hold keys, builds and the watch; no second signer | P16, P9, P6 | The Bybit origin. One compromise, all paths | Off-host release signing; hardware keys; second approver for release and workflow changes | Tabletop drill (**external test pending**) |
| HIGH | `l1-wallet` signs after printing, with no prompt; collector not pinned in CLI; auto-fee up to `u64::MAX` base units | P4 | Same binary the owner uses; drains by fee | Confirm prompt; pin `FEE_COLLECTOR`; fee cap | `l1_wallet_refuses_unpinned_collector`; `l1_wallet_send_requires_confirmation_without_yes` |
| HIGH | Testnet genesis unpinned; signing trusts the node's genesis | P3 | Signature valid on a chain the attacker controls | Pin testnet genesis; refuse unpinned on sign | `sign_terms_refuse_unpinned_genesis` |
| HIGH | Mainnet genesis not yet pinned (`MAINNET_GENESIS` empty) | P3 | Mainnet check cannot refuse a real genesis until the ceremony | Fill at ceremony; test that the pin is non-empty on the mainnet build | `mainnet_build_has_pinned_genesis` |
| HIGH | Hub runs any binary named in settings; key path unchecked | P5 | Settings edit or UI change runs code | Bundle-restricted or hash-pinned miner; key ownership check | `hub_refuses_miner_outside_bundle` |
| HIGH | Site build job holds `id-token: write`; `npm ci` runs lifecycle scripts; build writes the Pages artifact | P8 | Malicious package reaches origin deploy | Least-privilege permissions per job; `npm ci --ignore-scripts` with allow-list | Workflow lint fails on `id-token: write` in build |
| HIGH | Installers not OS-signed; Windows and macOS not reproducibility-checked; reproducible build not required | P6, P15 | Users click through warnings; the release cannot be rebuilt on two of three platforms | Authenticode and notarization off-runner; reproducible check as a required check on tags | `gh attestation verify` step; required-check list includes reproducible build |
| HIGH | Updater not live; enabling it without key-isolation and rollback refusal | P7 | Would push one bad build to every install | Offline signer, two-person presence, monotonic version check | `updater_refuses_older_version` |
| HIGH | Validator keys: plaintext loaded on testnet; Windows relies on ACL; BitLocker off at audit time; mainnet planned from the PC | P9 | Live chain equivocation or key theft | Sealed keys on every network that holds value; full-disk encryption verified; key off the PC before mainnet | `validator_refuses_plaintext_on_mainnet_config` |
| HIGH | custody-mpc combiner sees the key and message; no isolation; custodians' summary not verified against bytes (unverified in code read) | P10 | Custody at scale is the Bybit shape | Summary-to-bytes check in combiner; isolated host or TEE; written residual | `combiner_refuses_message_not_matching_approved_summary` |
| MEDIUM | `cargo vet` report mode, 1068 exemptions | P8 | Unreviewed crates reach binaries | Blocking for core crates before the rest | `cargo vet check --locked` on core set |
| MEDIUM | `ci.yml` installs playwright with `--no-save`, no lockfile | P8 | Build tooling, not shipped code | Lockfile for the install | Lockfile present and used |
| MEDIUM | Single CODEOWNER; "two reviewers" not enforced | P6, P16 | Review is one person | CODEOWNERS path rules once a second reviewer exists | Ruleset export committed (**owner-attested** until then) |
| MEDIUM | Vault actions not shown in wallets; guardian key custody undocumented | P11 | Cancel window invisible at signing | Vault summary in core; document custody | `vault_summary_states_delay_and_limit` |
| MEDIUM | Council pause and gate 10 (committee capture) open; signer set not in repo | P12 | Halt or capture is a governance-level risk | Publish signer set; two-person confirmation; close gate 10 | Pause refusal test |
| MEDIUM | Explorer is a viewer served through the CDN, not a verifier | P14 | Users treat it as truth | Show txid and bytes hash; label it | Mismatch fixture fails CI |
| MEDIUM | Ledger not shipped; host-device mismatch not fuzzed | P13 | Not live; the device is the right anchor | Fuzz the review path before a release | Host-vs-device fixture |
| MEDIUM | Incident notice is a draft; rotation and 2FA claims are owner-attested | P2, P15 | Claims readers rely on are unverified in the repo | Publish after the owner's sign-off; attach dated evidence | Sign-off checklist in the notice header |

## 5. Work order (work list for pre-mainnet hardening)

1. CRITICAL 1 and 2: the preview-binding and clear-sign wiring in `apps/wallet-gui`. These are the Bybit fix and they are code work, not policy.
2. CRITICAL 3: move the integrity watch off the PC and add the build-vs-live CI job; explain or rule out the 2026-10-13 diff in writing.
3. HIGH items for `l1-wallet` and the testnet genesis pin: small, testable, same class.
4. CRITICAL 4 is an operational decision for the owner: off-host signing and a second approver. Recorded as a decision, not a code change.
5. Everything else in the table, in rank order, with its test written before its code (TDD, per `rules/00-core.md`).

Each item above needs its test to fail on the current code before the fix. Until
then the item is open. Nothing in this document is a claim that a test passed.

## 6. External test pending

- Independent code review of `apps/wallet-gui` signing: **external test pending**.
- Independent review of `crates/custody-mpc`: **external test pending**.
- Penetration test of maya2c.dev and the CDN configuration: **external test pending**.
- Independent review of the release pipeline and signing: **external test pending**.

No external pentest or audit has been commissioned or performed. Per the owner's 2026-10-28 instruction, no third-party review is commissioned before
mainnet; this section records these as open, not done.
