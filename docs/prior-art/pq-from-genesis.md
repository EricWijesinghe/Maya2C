# Prior art: Post-quantum signatures from genesis

**Search status:** searched 2026-09-26.

**Who else does something similar.** QRL has used XMSS (hash-based, stateful) since its June 2018 genesis and is adding ML-DSA ([QRL docs](https://docs.theqrl.org/what-is-qrl/)).

**How Maya2C differs.** Maya2C uses a stateless hybrid, ML-DSA-65 and SLH-DSA-SHA2-128s, where both must verify (TX-1). It is not the first post-quantum chain.

**What may be said publicly.** "first post-quantum L1" is **forbidden**. "hybrid lattice + hash-based signatures on every transaction from genesis" is accurate and needs no superlative.
