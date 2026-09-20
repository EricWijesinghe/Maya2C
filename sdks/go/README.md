# sdks/go

Empty. There is no Go SDK.

The foundation brief lists `go` under `sdks/`; it also says not to build
product features in this phase. This is the reserved directory.

Same constraint as `sdks/python`: the hybrid signature is not reimplemented.
`sdks/sdk-ffi` is a `cdylib` with a C ABI, which cgo can call, and that keeps
one implementation of the thing consensus checks. A pure-Go signer would be a
second implementation of a consensus rule - `docs/invariants.md`, invariant 2,
records what this project already pays to avoid exactly that.
