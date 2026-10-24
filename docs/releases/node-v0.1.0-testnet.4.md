# Maya2C node-v0.1.0-testnet.4: 256-bit amounts, sealed keys, wallet recovery

**Replace testnet.3.** maya-testnet-1 was re-genesised on 2026-10-17 for
256-bit amounts, so a testnet.3 node can no longer join it. This release
matches the chain that is running now. It is a testnet release: nothing on
the chain carries value.

> **Testnet, unaudited.** No external security audit has reported yet, and
> mainnet stays blocked until one has. The installers are not code-signed, so
> Windows and macOS warn about an unknown publisher. Verify every download
> against `SHA256SUMS`.

## What's new since testnet.3

- **Every amount is 256-bit** (ADR-046/048): balances, fees, bonds and the
  wallet's send screen. A new genesis was needed, so old nodes and old wallet
  builds do not match the running chain.
- **Sealed validator keys** (ADR-049): a production node writes and reads the
  validator key only encrypted, with `--validator-passphrase-file`.
- **Post-quantum transport:** peers prefer ML-KEM-1024 and still accept 768.
- **A restarted validator rejoins** instead of following forever (#145).
- **Maya Wallet 0.1.1:**
  - recovery from your 24 words is now reachable from the Unlock screen
    ("Forgot passphrase? Recover with your recovery phrase");
  - before, it was shown only on first run, so a reinstall that found an
    earlier wallet left no way to recover.
- **Supply chain:** release builds carry an SBOM and a provenance attestation
  (`gh attestation verify`).

## Upgrading the wallet

Wallet 0.1.1 installs over 0.1.0. Your keys stay in your operating system's
keychain and your 24 words still recover the same accounts.
