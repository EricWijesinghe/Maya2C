# 02 — Cryptography

What Master Prompt 2 asked for, what this tree now does, and the output of the
commands that establish it. Every number here was produced on this machine by a
command named beside it. Where something could not be measured, that is said
rather than estimated.

**Machine:** Windows 11, x86-64, `nightly-2026-07-15`, release profile unless
stated. Kani, Speculos and the Ledger build run in WSL Ubuntu 24.04.

## 1. Where it stands

| Brief item | State | Evidence |
|---|---|---|
| Signature suites `0x01/0x10/0x11/0x20/0x21/0x30`, governance default, agility | **Live from genesis** (2026-09-27); `0x01` refused under mainnet rules | §3, ADR-007, ADR-013 |
| KEMs: ML-KEM-768/1024, HQC-128/256 (draft), DualKem, X-Wing | Built; HQC off by default | §4, ADR-009 |
| ArgonBlake | Already the consensus PoW before this work | §8 |
| Secret memory safety: zeroize, secrecy, subtle, dudect | Built; **one real leak found** (HQC), one harness artefact fixed | §5 |
| Entropy: SP 800-90A DRBG, 90B health tests, SIM sources, SP 800-22, Dieharder | Built, RESEARCH | §6, ADR-010 |
| Transparent ZK: no Groth16/BN254/trusted setup | Done; the pairing pool is deleted | §7, ADR-008 |
| Threshold custody: m-of-n REAL, threshold lattice RESEARCH | m-of-n **REAL**, spends through the apply path; threshold lattice is **Threshold Raccoon** (RESEARCH), bit-exact with the authors' reference, dealerless keygen | §9, ADR-011, ADR-013 |
| HTLC: hash-locks REAL, Module-LWE RESEARCH | Hash locks **live from genesis** | §10, ADR-012 |
| Archival: forward-secure keys, SLH-DSA seals, 100 epochs | Built, RESEARCH | §11 |
| Ledger app: APDUs, RAM measurement, Speculos | Signs suite `0x10`; 8/8 on two devices | §12, `docs/ledger-feasibility.md` |
| `benches/crypto.rs`, CSV/Markdown export | Done | §2 |

Two things in the brief turned out to be false or impossible as stated, and
are reported rather than worked around. (A third — "m-of-n multisig REAL
now", which could not be REAL while the v7 envelope was dark — was resolved on
2026-09-27 by ADR-013, which met ADR-007's three activation preconditions and
moved the height to genesis. `crates/node/tests/suite_envelope_live_tests.rs`
moves value with a v7 transfer and a 3-of-5 spend through `apply_block` at the
node's own context.)

1. **"If the Ledger cannot fit ML-DSA, target Stax/Flex."** Stax and Flex have
   *less* app RAM (36 KiB) than a Nano S Plus (40 KiB). The fallback does not
   exist; a low-memory signer was written instead.
2. **"True threshold lattice signing (DKG + partial signature aggregation),
   enable only after a peer-reviewed scheme is chosen."** The two conditions do
   not meet in the literature. The peer-reviewed scheme chosen (Threshold
   Raccoon, EUROCRYPT 2024; ADR-014) assumes a *trusted dealer* -- its paper
   puts a DKG "outside of the scope of this work" -- and a dealer holds the
   whole key, which the brief forbids. So signing is the peer-reviewed scheme,
   and the dealerless keygen is this repository's own construction, labelled
   as not peer-reviewed. Separately, a Threshold Raccoon signature is a
   *Raccoon* signature, which is not a NIST standard, so it authorises custody
   off chain and signs no transaction.

## 2. Benchmarks

`MAYA_CRYPTO_REPORT=1 cargo bench --bench crypto` — 44 rows, every one exported
to `reports/crypto-bench.csv` (raw nanoseconds) and `reports/crypto-bench.md`
(the full table, plus the size tables). The export is written from criterion's
own `estimates.json`, and this section is rebuilt from that CSV by script, so
nothing in it is retyped by hand. Re-run 2026-09-27, all rows in one run.

Means, 20 samples each, release profile, this machine:

| Suite | keygen from seed | sign | verify | Public key | Signature |
|---|---|---|---|---|---|
| Ed25519 (`0x01`) | 15.90 µs | 13.41 µs | 32.49 µs | 32 B | 64 B |
| ML-DSA-65 (`0x10`) | 155.23 µs | 252.57 µs | 80.19 µs | 1,952 B | 3,309 B |
| ML-DSA-87 (`0x11`) | 299.36 µs | 720.40 µs | 210.31 µs | 2,592 B | 4,627 B |
| SLH-DSA-SHA2-128s (`0x20`) | 17.24 ms | 162.11 ms | 146.74 µs | 32 B | 7,856 B |
| SLH-DSA-SHAKE-256f (`0x21`) | 6.80 ms | 153.86 ms | 4.69 ms | 64 B | 49,856 B |
| Hybrid (`0x30`) | 13.17 ms | 135.35 ms | 211.71 µs | 1,984 B | 11,165 B |

| KEM | generate | encapsulate | decapsulate | Encapsulation key | Ciphertext |
|---|---|---|---|---|---|
| ML-KEM-768 | 39.20 µs | 35.76 µs | 33.77 µs | 1,184 B | 1,088 B |
| ML-KEM-1024 | 50.68 µs | 37.38 µs | 56.77 µs | 1,568 B | 1,568 B |
| HQC-128 (draft) | 71.33 µs | 248.67 µs | 369.12 µs | 2,241 B | 4,433 B |
| HQC-256 (draft) | 426.45 µs | 829.27 µs | 1.82 ms | 7,237 B | 14,421 B |
| X-Wing | 39.63 µs | 93.25 µs | 84.47 µs | 1,216 B | 1,120 B |
| Dual ML-KEM-768+HQC-128 | 97.25 µs | 227.33 µs | 311.62 µs | 3,425 B | 5,521 B |
| Dual ML-KEM-1024+HQC-256 | 464.78 µs | 1.38 ms | 1.90 ms | 8,805 B | 15,989 B |

| Hash | Mean |
|---|---|
| ArgonBlake, one 64-byte header (consensus PoW) | 23.78 ms |
| BLAKE3, 64 B | 123 ns |
| SHA-256, 64 B | 71 ns |
| SHA3-256, 64 B | 401 ns |
| kHeavyHash, per nonce (Kaspa PoW, *reference*, see below) | 522 ns |

What these numbers decide elsewhere in this report:

- **The hybrid costs 135.35 ms to sign and 211.71 µs to verify.** Signing is
  dominated by its SLH-DSA half, which is why a Ledger cannot produce one (§12)
  and why the `0x10` suite exists at all.
- **Verification is what consensus pays**, and there ML-DSA-65 (80.19 µs) is
  cheaper than the hybrid (211.71 µs) and 59× cheaper than SLH-DSA-SHAKE-256f
  (4.69 ms). A block of 10,000 `0x21` verifications would be 47 seconds;
  that suite is for cold vaults and archive seals, not transactions.
- **SLH-DSA-SHAKE-256f signing is 153.86 ms**, which is the cost behind the
  100-epoch archive test in §11 (100 keygens plus 200 signatures).
- **HQC-128 decapsulation costs 11× ML-KEM-768's** and its ciphertext is 4×
  larger, before §5's timing finding. The dual handshake pays both.
- **ArgonBlake is ~23.78 ms per attempt**: memory-hard by design, and the reason
  `[profile.dev.package.*]` overrides exist (ADR-003).
- **kHeavyHash is 522 ns per nonce — for this reference, not for Kaspa.** The row
  times `benches/support/kheavyhash.rs`, a port of rusty-kaspa's own code checked
  against Kaspa's published known answers (`heavy_hash`, matrix generation) and
  against cSHAKE256 re-derived from NIST SP 800-185
  (`crates/node/tests/kheavyhash_reference_tests.rs`); the bench refuses to run
  if the known answer fails. Upstream uses an assembly Keccak on x86-64 and
  miners use GPUs, so this says what the construction costs on one core here,
  not how fast Kaspa mines. Beside ArgonBlake it is ~45,555× cheaper per
  attempt: kHeavyHash is compute-bound by design, ArgonBlake memory-bound.

Caveats a reader should apply: 20 samples on a Windows laptop, so the standard
deviations are wide (up to 21% of the mean for the hybrid's signing) and these
are *relative* costs, not datasheet figures. Energy is absent — §13 says why.

## 3. Signature suites and crypto-agility

`cargo nextest run -p maya-crypto-pq`

- **NIST ACVP vectors, vendored** from ACVP-Server
  `975de31eb83d87039ec88934fdc47d8c312b892d`, with upstream SHA-256 and trimmed
  size recorded per file in `tests/vectors/acvp/MANIFEST.txt`:

  | Algorithm | keyGen | sigGen / encapDecap | sigVer |
  |---|---|---|---|
  | ML-DSA (FIPS 204) | 20 | 40 | 60 |
  | ML-KEM (FIPS 203) | 20 | 80 | — |
  | SLH-DSA (FIPS 205) | 20 | 28 | 36 |

- **Suites below 128 bits of PQ security are named in the registry, not
  assumed:** `Ed25519` carries `pq_security_bits = 0` and
  `mainnet_allowed = false`, and the agility audit reports it. The `0x30`
  hybrid records the *weaker* half (128 bits), so the audit errs safe.
- **Migration:** `crates/crypto-pq/tests/agility_migration_tests.rs` moves
  **1,000,000 accounts** through a deprecation window: 500 window blocks, 41
  sweep blocks, no failed transfer. The authorization in that simulation is a
  keyed hash, not a signature — a SIM, and labelled one; unit tests cover the
  real signature path.
- **Parity:** suite `0x30` is byte-for-byte the node's existing hybrid
  (`crates/node/tests/suite_parity_tests.rs`), which is what lets the envelope
  land without migrating an account.

## 4. KEMs

`cargo nextest run -p maya-crypto-pq --test kem_kat_tests`

| KEM | Checked against |
|---|---|
| ML-KEM-768, ML-KEM-1024 | NIST ACVP (FIPS 203) |
| HQC-128, HQC-256 | the reference implementation's KATs — **draft standard**, labelled so everywhere |
| X-Wing | the draft's own vectors |
| DualKem (ML-KEM + HQC, SHA3-256 combiner) | its halves, plus a test that a broken half breaks the whole |

HQC is behind an off-by-default feature; the node opts in only for its
ephemeral dual handshake. §5 says why that matters.

## 5. Secret memory safety, and the leak dudect found

`bash scripts/dudect.sh` — full output in `reports/dudect.txt`. (Not
`cargo bench`, which appends a `--bench` argument dudect-bencher rejects.)

Compile-time checks assert that no secret type implements `Display`, `Copy` or
`Clone` where it must not, and that every one is `ZeroizeOnDrop`. The master
seed lives in `secrecy::SecretBox`; secret comparisons go through `subtle`.

The timing harness reports Welch's *t* over two input classes. Its two controls
establish that it works: a deliberately naive byte-by-byte compare shows
*t* = 473.8 (leaking, as intended), and a null test of the same function
against itself shows *t* = 2.5 (no signal).

Re-measured 2026-09-27 on an idle machine with the corrected harness (below);
`reports/dudect.txt` is that run, exactly as printed.

| Operation | max &#124;t&#124; | n | Reading |
|---|---|---|---|
| `ct_eq` on a shared secret | 1.5 | 87k | constant-time |
| ML-KEM-768 decapsulation | 2.4 | 18k | constant-time |
| ML-KEM-768 decapsulation (null control) | 2.0 | 13k | — |
| X-Wing decapsulation | 1.8 | 96k | constant-time |
| ML-DSA-65 verify | 2.4 | 39k | constant-time |
| Ed25519 sign | 1.5 | 39k | constant-time |
| **HQC-128 decapsulation** | **10.3** | 2k | **leaks** — confirmed below |
| HQC-128 decapsulation, continuous | **47.5** | 176k | **leaks**, effect size tau = 0.11 |
| HQC-128 decapsulation (null control) | 2.3 | 25k | — |
| ML-DSA-65 sign | 23.8 | 3k | see below |
| naive compare (positive control) | 525.9 | 18k | leaking, as designed |

What changed, and what did not:

- **The first HQC figure was partly the harness.** The original run reported
  |t| = 26.9 at n = 3k. Its two classes took their ciphertext buffers from
  different allocation paths -- one straight out of `encapsulate`, the other
  from a fresh `Vec` -- which puts them in different places in memory, a
  difference the timer can see and the algorithm does not have. With both
  classes allocated by the same statement the fixed-size run reads 10.3.
- **The leak underneath is real.** A five-minute continuous run of that bench
  alone reaches |t| = 47.5 at n = 176k with a stable effect size (tau = 0.11).
  A leak's t grows with the square root of n; noise's does not. Two earlier
  continuous runs reported tau near 0.7, but they had overlapped each other on
  this machine, so they are not used here -- contention inflated them.
- **Where it is, is not known.** Reading `hqc-kem 0.1.0-rc.0`'s decapsulation
  found no variable-time step: the ciphertext comparison folds every byte with
  no early exit, Berlekamp-Massey and Forney run fixed bounds with masks,
  `gf_mul` and `gf_inverse` are table-free, the encryption path samples with
  the fixed-cost modular method, and the rejection sampler is keygen-only.
  Finding it needs timing probes inside a vendored copy, which this report did
  not do. The mitigation is unchanged and is why the number matters less than
  it looks: HQC is off by default, draft-labelled, and never the only KEM -- in
  the dual combiner an attacker who recovers the HQC half still faces ML-KEM.
- **ML-DSA-65 signing** is expected to differ: FIPS 204 signing is a rejection
  loop whose *number of iterations* depends on the key and the message, and the
  classes are one fixed key against a pool of 32. What must not vary is
  verification, and that measures 2.4.

## 6. Entropy

`cargo nextest run -p maya-entropy`, then `bash scripts/entropy_battery.sh`.

- **HMAC-DRBG (SP 800-90A Rev. 1 §10.1.2)** against the NIST CAVP
  `HMAC_DRBG.rsp` vectors: **480 of 480** cases.
- **SP 800-90B health tests** on every source, every read: repetition count and
  adaptive proportion, cutoffs computed from the declared min-entropy at
  α = 2⁻²⁰ and checked against Table 2.
- **SP 800-22 (NIST STS)**, 100 streams of 1,000,000 bits, flagged tests out of
  ~188 (`reports/entropy/sts-*.txt`):

  | Source | Flagged |
  |---|---|
  | pool (DRBG output) | **0** |
  | OS (`getrandom`) | 2 |
  | RDSEED | 4 |
  | sim-thermal | 1 |
  | sim-homodyne-QRNG, raw | 154 |

  The last is the point of the exercise: raw Gaussian ADC samples from the
  simulated homodyne detector are *not* uniform, and the harness reports that
  rather than hiding it behind conditioning.
- **Dieharder 3.31.1** over the pool, piped as a stream so the long tests do not
  re-read one finite file: **114 PASSED, 0 WEAK, 0 FAILED**
  (`reports/entropy/dieharder-pool.txt`).
- Every simulated source names itself `sim-` and says so in its logs; Casimir is
  RESEARCH and produces nothing.

## 7. Transparent zero knowledge

`cargo nextest run -p maya-zk-stark -p custom-l1-node`

The brief's requirement was "no Groth16, no BN254, no trusted setup". Both
pairing-based systems are **deleted**, not disabled: `crates/zk-privacy`
(Groth16 over BLS12-381) and `crates/zkml` (halo2/KZG over BN254) are gone,
along with every arkworks dependency.

- **Shielded pool:** one 2-in-2-out joinsplit AIR, 2⁸ rows, Plonky3 uni-STARK
  over BabyBear with hiding FRI and Keccak commitments. Proof **≈ 383 KB**
  (382,689 B and 382,703 B measured on two runs — hiding randomness varies it),
  **113 bits conjectured / 99 proven** from Plonky3's own estimator.
- **The gate:** `crates/zk-stark/tests/pqc_zk_tests.rs` runs `cargo tree` over
  the workspace (all edge kinds, offline) *and* scans `Cargo.lock` for every
  other target. It fails if `ark-groth16`, `ark-bn254`, `ark-bls12-381`,
  `ark-snark`, any `halo2*`, `bls12_381`, `bellman` or `snark-verifier`
  reappears.
- **Under-constraint negatives:** every constraint has a consistent-lie test —
  the trace is recomputed from the lie so only the constraint under test can
  refuse it — and the suite passes in release builds too, where the prover's
  debug assertions are gone and only the verifier can refuse.
- Credential disclosure and sanctions non-membership moved to STARKs as well, so
  an identity presentation is now post-quantum.
- **Mainnet stays blocked**, on a different reason than before: the circuit has
  had no independent audit (`pool::CIRCUIT_IS_AUDITED = false`), and an
  under-constrained AIR mints hidden supply exactly as a subverted setup would.

## 8. ArgonBlake

`cargo bench --bench argon_blake`, and the `hash` group of §2.

ArgonBlake predates this work: it is the chain's consensus proof-of-work and
was not changed here. What this report adds is a current measurement beside
every other primitive, on the same machine and profile.

Three stages: a BLAKE3 pre-hash of the header, **Argon2id over 32 MiB**, then a
BLAKE3 XOF squeeze. The measurement says where the cost is: the whole hash is
**23.78 ms** and BLAKE3 over the same 64 bytes is **123 ns**, so the memory-hard
stage is essentially all of it — which is the point of the construction, and why
no SIMD tuning of the BLAKE3 halves would matter.

The module's own doc records ~25.4 ms from an earlier run, and the first run of
this bench measured 30.71 ms; this one reads 23.78 ms with a 4.56 ms standard
deviation over 20 samples. The spread across runs is the machine, not the
construction. None of these figures is a datasheet number.

## 9. Threshold custody

`cargo nextest run -p maya-crypto-pq -p maya-custody-mpc -p custom-l1-node`

- **m-of-n multisig** is a policy account: up to 16 post-quantum suite keys,
  and the account *is* the policy's address, so a spend must present the policy
  the address commits to. Strictly increasing approvals make a repeated signer
  unrepresentable; every presented signature must verify. 3-of-5 tested with two
  members offline, at the policy level and on the node (wire v8), and since
  ADR-013 through `StateDB::apply_block` at the production context: the quorum
  moves 400 units, two approvals move nothing.
- A review found **txid malleability** here: hashing the approvals gave one
  spend as many transaction ids as it has quorums. The v8 txid now covers the
  signing bytes only, pinned by a test with two quorums and a superset.
- **Custody transport** now negotiates `X25519MLKEM768` (draft-ietf-tls-ecdhe-mlkem,
  codepoint `0x11EC`) and nothing else, so a recording of today's session cannot
  be broken later. 3-of-5 over it survives a crashed custodian and a corrupted
  share, refuses three failures, and refuses a classical-only peer.
  The certificate is still classical (`webpki` verifies no ML-DSA
  certificate), but since 2026-09-27 authentication does not rest on it: each
  side signs the session's TLS exporter with a pinned ML-DSA-87 key
  (`custody-mpc::pq_auth`, ADR-011 amendment). A 3-of-5 ceremony signs through
  an impostor holding a valid certificate from the institution's own CA and a
  crash; a proof made for another session and an unexpected server key are
  refused.
- **Threshold lattice signing is Threshold Raccoon-128** (ADR-014), behind
  `threshold-lattice`. Ported from the scheme authors' own reference
  (`masksign/ec24-thrc`, commit `8ef114a`) and checked against it **bit for
  bit** over a 3-of-5 session with two parties offline: `A`, the key, all five
  shares, all 25 pairwise seeds, every commitment, mask, MAC and response, and
  the signature (`scripts/traccoon_vectors.py`, `tests/threshold_tests.rs`).
  The reference's 256-bit Gaussian samples are the one input replayed rather
  than recomputed, and the test checks each is requested with the reference's
  seed and width. That test caught a real bug on its first run: the reference
  samples `A` in the NTT domain, and the port had taken those values as
  coefficients -- a valid key, just not the reference's.
- **The keygen is dealerless** (`threshold::dkg`, this repository's, not
  peer-reviewed): each party commits to its piece of the key before revealing
  it and Shamir-deals it, and the group secret is never summed anywhere. Over
  it, any three of five sign with two offline, two cannot, a crash after round
  one aborts the session and a retry signs, a lying signer is caught in round
  three, a session id is never answered twice, and a dealer who sends a bad
  share costs availability, not security.
- **One finding in the reference itself:** its modulus (Raccoon's, a product
  of two primes) misses the paper's own Lemma 3.2 condition at `nu_t = 37`.
  Worked out in ADR-014, the effect is a doubled rounding error on a
  coefficient with probability 9.1 × 10^-8, far inside the verification bound's
  slack.

## 10. HTLCs

`cargo nextest run -p maya-htlc-lattice -p maya-htlc-watcher -p custom-l1-node`

- **Hash locks are REAL and live from genesis** (`HASH_LOCK_ACTIVATION_HEIGHT = 0`):
  SHA3-256, BLAKE3 or SHA-256 over a 32-byte preimage. Height 0 reinterprets no
  history, because before this change every HTLC payload failed at execution.
- **Why a 256-bit hash lock is already post-quantum:** Grover's search needs
  about 2¹²⁸ *sequential* hash evaluations and parallelises badly (k machines
  buy √k). Collision search is irrelevant: the lock's creator fixes the digest
  from a preimage it chose, so there is no pair to collide.
- **SHA-256 was added** beyond the brief's SHA3-256 and BLAKE3, because it is the
  only one Bitcoin (`OP_SHA256`) and Ethereum (the `sha256` precompile) can
  check — a hash lock the other chain cannot match is not a swap. Its known
  answer for 32 zero bytes is pinned.
- Tested in the node's **production context, with no activation override**: a
  SHA-256 swap across two chains through the real watchers, refund at `T` with a
  late preimage as a no-op, forged preimages leaving the lock open, and a
  lattice lock refused where a hash lock is accepted.
- Module-LWE locks stay RESEARCH at `u64::MAX`.

## 11. Archival crypto

`cargo nextest run -p maya-archive --test seal_tests`

Forward-secure seals over archive roots: SLH-DSA-SHAKE-256f, one key per epoch,
the next seed `SHAKE256(domain ‖ seed)` and the old one zeroized by `evolve`
consuming the signer. Each epoch certifies its successor's public key before it
is erased, so a verifier holding only the genesis key follows the whole history
with no horizon fixed in advance.

- **100 epochs** sealed, evolved and verified from the genesis key alone, in
  86 s (debug-profile crypto with the release override; the figure is a cost, not
  a target).
- **Compromise at epoch 50:** the attacker seals as 50 and later, and cannot
  backdate a seal to 49 or certify a fresh key for an old epoch.
- `docs/resealing.md` is the plan for when an algorithm ages: wrap the old seal
  (seal suite), re-address under a new multihash with a cross-referencing seal
  (CIDs are BLAKE3-256), or start a new chain from the compromise epoch.

## 12. The Ledger app

`cargo test --manifest-path apps/ledger-maya2c/Cargo.toml`, then
`scripts/ledger_speculos.sh` (and `DEVICE=nanox`).

The full account is in `docs/ledger-feasibility.md`. In short:

- **Stock `fips204` cannot run on any Ledger.** Measured on the host: keygen
  ~143 KiB of stack, signing ~158 KiB, net of a 142 KiB baseline. App SRAM is
  40 KiB (Nano S Plus), 36 KiB (Stax, Flex), 28 KiB (Nano X).
- **A low-memory ML-DSA-65 fits.** `A` is never stored, `y`/`s1`/`s2` are
  regenerated, `w = A·y` is produced one row at a time. **≈ 23 KiB** peak stack
  on the device CPU from `-Z emit-stack-sizes` over the linked ELF
  (`reports/ledger/stack-sizes-thumbv8m.txt`).
- It is trusted because it is not a new function: byte-equal to **NIST ACVP**
  ML-DSA-65 (10 keyGen, 20 sigGen including hedged and both interfaces) and to
  `fips204` over 64 random keys and messages.
- **Speculos: 8 of 8** on Nano S Plus and on Nano X, API level 27 — including
  that the device's own SLIP-0010 derivation matches an independent host
  derivation of the same phrase, and that an approved transfer is signed to the
  library's exact bytes while a rejected one returns `0x6985`.
- The signature is suite `0x10`, not the hybrid, and since ADR-013 a v7
  transaction the chain accepts from genesis.

## 13. What is still open

- **The shielded circuit has had no independent audit.** Six guards refuse a
  value-bearing chain id while `CIRCUIT_IS_AUDITED` is false.
- **HQC decapsulation leaks timing** (§5).
- **Lattice HTLCs are dark** (ADR-012). The v7 envelope and multisig were
  activated at genesis by ADR-013.
- **`shielded.max_per_block` is governance-declared but unread** by the node,
  which enforces its own constant. Pre-existing; documented in
  `docs/governance.md`.
- **The threshold keygen is not verifiable** (ADR-014): a malicious dealer
  can deny service, and naming it needs lattice verifiable secret sharing.
  Threshold Raccoon's output is not a standard signature, so threshold custody
  still cannot sign a *transaction*; that needs a peer-reviewed threshold
  ML-DSA.
- **Custody certificates are classical**, and are no longer what
  authentication rests on: the ML-DSA-87 exporter signature is (ADR-011
  amendment).
- **No energy figures.** Every joule number would be measured seconds times an
  assumed wattage; RAPL is not readable on this machine.
