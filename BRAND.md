# Brand and Trademark Policy

The **source code** of Maya2C is licensed under the Apache License 2.0
([LICENSE](LICENSE)). Anyone may copy, modify, run and redistribute it under
those terms. This document is different: it covers the **names, marks and
visual identity** of the project, which the license does not grant.

## What is covered

- The name **Maya2C** (one word, always; never "Maya 2C" or "Maya-2C").
- The **MAYA** ticker.
- The Maya2C emblem, the wordmark, and the **Forged Light** brand assets
  (`docs/brand/`, the palette, typefaces and site components).
- Any logo, banner or icon derived from the above.

These are trademarks of Eric Wijesinghe, the project maintainer. Nothing in
the Apache License 2.0 grants a right to use them.

## What you may do without asking

- Refer to the project factually: "forked from Maya2C", "compatible with the
  Maya2C source", "based on the Maya2C code". Link to the upstream repository.
- Use the name in plain text to describe your own writing about the project
  (reviews, research, news), without implying endorsement.

## What you may not do

- Use "Maya2C", "MAYA", the emblem, the wordmark or Forged Light assets in the
  name, domain, token name, ticker, logo or marketing of a fork, a network, a
  wallet, an exchange listing or any other product.
- Present a fork, a testnet, a relaunch or any other network as **"the" Maya2C
  network**, as official, or as endorsed by the maintainer.
- Imply affiliation, partnership, audit, or certification that does not exist.
- Use a confusingly similar name (for example a one-letter change, a
  different suffix, or the same name in another language or script).
- Reuse the **mainnet chain tag, genesis file, genesis hash, or the MAYA
  ticker** for a chain that is not the canonical network.

## Forks must rename and rebrand

If you fork the code, you must:

1. Choose a new project name and ticker that do not resemble Maya2C or MAYA.
2. Replace the logo, wordmark and Forged Light assets with your own.
3. Generate your own chain tag and genesis. Do not copy the canonical ones.
4. Remove or rewrite the "Maya2C" name in user-facing text, binaries and
   default configuration where it would suggest the canonical network.

Keeping the Apache-2.0 notices and attribution in the source is required by the
license and is not a trademark use.

## The canonical network

The canonical Maya2C network is the one whose **genesis hash and validator set
the maintainer publishes** (see [docs/GENESIS.md](docs/GENESIS.md) and the
project's release notes). The source code does not define the network: anyone
can build the same source, but a build of the code does not make a network
canonical.

Protocol-level separation is in place: the Noise handshake is bound to the
chain tag (commit 23482ba6, PR #377), so a node on a different genesis fails
the handshake and cannot exchange gossip with the canonical network. A fork
therefore starts as a separate, empty chain with no users, stake, history or
coin of the canonical network.

## Permission

To ask for permission to use a mark beyond what is listed above, open an
issue on the repository. Permission, if granted, will be in writing and
limited to the stated use. Silence is not permission.

This policy is not legal advice. It states the maintainer's position on the
marks; it does not change the Apache-2.0 license for the source code.
