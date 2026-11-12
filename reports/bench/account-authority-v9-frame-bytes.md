# Wire version 9 frame size (ADR-086 §3.2)

Encoded size, not time: deterministic, so hardware does not matter.
Branch `postgen/account-authority`, base `027c7813`, 2026-11-10.

Command:

```
cargo test -p custom-l1-node --test account_authority_tests measured_v9_frame_bytes -- --nocapture
```

Output (verbatim):

```
v9 frame, suite 0x30 2-of-3, one output: 28420 bytes (v6 single-key transfer: 13239 bytes)
v9 frame, suite 0x30 3-of-5, one output: 43560 bytes (v6 single-key transfer: 13239 bytes)
```

Device keys are suite `0x30` (hybrid ML-DSA-65 + SLH-DSA-SHA2-128s), the
wallet plan in ADR-086 §8. Every listed key travels in the frame (1,984 bytes
each) and every approval carries an 11,165-byte hybrid signature, so size
grows with both `n` and `m`. Against `MAX_BLOCK_BYTES` (5 MiB, invariant 33) a
block holds about 184 2-of-3 frames or 120 3-of-5 frames; the per-byte fee
already prices this. No target is asserted.
