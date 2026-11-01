#!/usr/bin/env bash
# Windows Authenticode placeholder. No certificate exists yet, so with no
# WINDOWS_CODESIGN_PFX_BASE64 secret the release is unsigned and says so in the
# log. If the secret is added before the signing step is written, fail the job
# rather than publish something that looks signed.
set -euo pipefail
if [ -z "${PFX:-}" ]; then
  echo "UNSIGNED: no key (WINDOWS_CODESIGN_PFX_BASE64 not set); Authenticode skipped"
  exit 0
fi
echo "::error::WINDOWS_CODESIGN_PFX_BASE64 is set but Authenticode signing is not wired"
exit 1
