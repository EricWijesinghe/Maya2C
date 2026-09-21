# Vendored test vectors

| Directory | Source | Trimmed | SHA-256 of the untrimmed source |
|---|---|---|---|
| `acvp/` | NIST `usnistgov/ACVP-Server`, `gen-val/json-files/*/internalProjection.json` | our parameter sets only; see `acvp/MANIFEST.txt` | per file, in `acvp/MANIFEST.txt` |
| `hqc/hqc-1.rsp` | HQC reference KATs (HQC-1 = HQC-128), as shipped in `hqc-kem 0.1.0-rc.0/kat/` (reference implementation commit `161cd4f`, 2026-02-10) | first 10 of 100 vectors | `f4135530c7c6bab0d2a49eca78118310c06721518d8df3774cc5201e66ae9cd2` |
| `hqc/hqc-5.rsp` | as above, HQC-5 = HQC-256 | first 10 of 100 vectors | `68d45adf1528f09554c452a5cde29929f73369b1d5374835252118c55541af5a` |
| `xwing/test-vectors.json` | draft-connolly-cfrg-xwing-kem `spec/test-vectors.json`, as shipped in `x-wing 0.1.0/tests/` | none | `a8726596f4c7629590f727b1bbeb483f6932292fbf5fd85c9cbb803190014f00` |

HQC is a **draft** standard: its KATs are the reference implementation's,
not NIST CAVP's, because no FIPS 207 validation program exists yet.
