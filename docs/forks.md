# Forks: what protects the real network

Maya2C is open source under Apache-2.0. Anyone can copy the code, and that is
intended. This page says plainly what stops a copy from passing itself off as
Maya2C, and what does not.

## 1. Copying the code is allowed

The source is public so that people can read it, audit it, build it and
improve it. A copy of the repository is legal and expected. We do not try to
prevent it, and no part of the software tries to.

## 2. A copy is a separate, empty chain

Forking the code gives you a binary and a blank chain. It does not give you:

- the canonical chain's **history** or its blocks,
- its **users**, accounts or balances,
- its **validators** or their stake,
- its **coin** (MAYA),
- its **genesis** and the keys that were generated for it.

Whoever runs a fork must start a new genesis and find their own validators and
users. The fork is a new network with no connection to the canonical one.

## 3. The network magic refuses cross-network connections

Every node binds its peer-to-peer handshake to its chain tag (Noise prologue,
commit 23482ba6, PR #377). A node on a different genesis fails the handshake
before any protocol message is exchanged. Two networks cannot silently mix
gossip, and a fork cannot pose as the canonical network to our nodes.

## 4. The brand and trademark terms

The license grants the code, not the name. [BRAND.md](../BRAND.md) states that
the Maya2C name, the MAYA ticker, the emblem, the wordmark and the Forged Light
assets may not be used by a fork, and that a fork must rename and rebrand.
This gives the project a legal basis to ask a confusingly named clone to
change its name. It is a notice, not a technical lock: it works by making the
terms clear and by enforcing them through the usual channels.

## 5. Signed releases, and an out-ship advantage

The real network can out-ship a fork. Signed releases and a signed update
channel (see [in-app-updates.md](in-app-updates.md)) let users verify that a
binary came from the maintainer. The Sentinel Recovery Network proposal
(MIP-0001, ADR-077) is designed to help the canonical network recover from
faults. Neither is a guarantee that a fork cannot be built, but both give users
a way to tell the real release from a copy.

## What does NOT work

**Self-destruct or anti-tamper code does not protect the network.** It is
malware by definition and would only harm legitimate operators:

- A check that "fires" in a modified binary can be removed by whoever runs the
  modified binary. It can only ever run on the software of honest nodes, which
  is the software that is supposed to keep working.
- Code that disables or damages a node when it detects an unknown genesis
  would hurt the people running the real network, and it would be a backdoor.
- Such code would break the open-source contract and cannot be justified as a
  security control.

We will not add it. The protections above are the ones that work.
