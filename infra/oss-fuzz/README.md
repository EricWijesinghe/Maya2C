# OSS-Fuzz submission for Maya2C

These three files are the OSS-Fuzz project definition for Maya2C:

- `project.yaml`: language rust, sanitizer `address`, engine `libfuzzer`.
- `Dockerfile`: starts from `gcr.io/oss-fuzz-base/base-builder-rust` and clones
  this repository at build time.
- `build.sh`: runs `cargo fuzz build` in `fuzz/` and copies every target in
  `fuzz/fuzz_targets/` into `$OUT`.

Nothing here is submitted automatically. Submitting is a manual step for the
owner.

## Eligibility: read this first

OSS-Fuzz accepts projects that have "a significant user base and/or are
critical to the global IT infrastructure" (the accepting-projects page). Maya2C
is pre-mainnet with no user base yet, so **the application may be declined**.
Submit it anyway only as a test of the process, or after mainnet. Do not
describe the project as widely used in the application.

Other requirements:

- `primary_contact` must be an address that is a committer in the git log
  (`git log --format=%ae`) and is linked to a Google account. A non-Google
  alias only gets bug-tracker access, not the ClusterFuzz dashboard. If the
  address is not the submitter's own, an informal email verification follows.
- Project name: letters, digits, underscores and dashes only. The name used
  here is `maya2c`.
- The application is a pull request, so every new file needs an Apache 2.0
  header dated to the current year (see the step below).

## Steps

1. Fork `google/oss-fuzz` on GitHub and clone the fork.
2. Replace the placeholder in `project.yaml`:
   `primary_contact: "REPLACE_WITH_GOOGLE_ACCOUNT_EMAIL"` with your Google
   account address. Do not commit a personal address to this repository; edit
   the copy inside the OSS-Fuzz fork only.
3. Copy the files into the fork:

   ```
   mkdir -p projects/maya2c
   cp /path/to/Maya2C/infra/oss-fuzz/{project.yaml,Dockerfile,build.sh} projects/maya2c/
   ```

4. Add the Apache 2.0 header to the top of each new file. Copy the header
   from an existing file in `projects/` in the fork, and update the year.
5. Test locally with Docker (run from the fork root):

   ```
   python3 infra/helper.py build_image maya2c
   python3 infra/helper.py build_fuzzers --sanitizer address maya2c
   python3 infra/helper.py check_build maya2c
   python3 infra/helper.py run_fuzzer --corpus-dir=/tmp/corpus maya2c <target>
   ```

   Run `check_build` before opening the PR. A build that fails here will fail
   the same way for OSS-Fuzz.
6. Commit in the fork and open a pull request against `google/oss-fuzz`.
7. After merge, builds usually start within a day. Crashes, coverage and stats
   appear on the ClusterFuzz web interface for the contact's Google account.

## Known toolchain issue

The repository root pins `nightly-2026-07-15` in `rust-toolchain.toml`. The
OSS-Fuzz builder uses its own nightly. `build.sh` deletes that file in the
throwaway checkout so rustup does not try to fetch the pin. The fuzz crate has
its own `[workspace]` and lockfile, so nothing else depends on the pin.

Whether `cargo fuzz build` succeeds on the builder's nightly is recorded in the
PR description, not here. See the verification notes there.
