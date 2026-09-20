# sdks/python

Empty. There is no Python SDK.

The foundation brief lists `python` under `sdks/`; it also says not to build
product features in this phase. This is the reserved directory.

When one is written, the decision that matters is made first: **do not
reimplement the hybrid signature.** A Maya2C signature is an ML-DSA-65 and an
SLH-DSA pair and both halves must verify, and a second implementation of that
is a second thing that can disagree with consensus. `sdks/sdk-ffi` already
exposes the real primitives over uniffi, which generates Python bindings -
that is the intended route, and `crates/node/tests/hybrid_parity_tests.rs` is
the shape of test any alternative would have to pass.
