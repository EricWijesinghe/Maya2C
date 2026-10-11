# Contributing

Maya2C is built and controlled by its maintainer. Outside contributions are
welcome as **suggestions**: issues, security reports, and pull requests from
forks. A suggestion is never merged automatically and never grants access.

## What happens to a pull request

1. It is inspected line by line: what it changes, why, and what it could
   break or open up.
2. The maintainer decides. Only the maintainer can merge; nobody else has
   write access, and no access is handed out.
3. CI on a pull request from a fork does not run until the maintainer
   approves the run, so a fork cannot use this repository's runners or
   secrets.

Most suggestions that are accepted are re-implemented by the maintainer rather
than merged as written.

## Rules GitHub enforces on `master`

- No direct pushes: every change arrives through a pull request.
- Required checks: reality ledger, spec conformance, rustfmt, clippy,
  nextest (core), cargo-deny.
- No force-push and no deletion.
- Release tags can only be created by the maintainer and can never be moved
  or deleted by anyone else.

## Releases

Only the newest release is kept published. Older releases are removed
when they are superseded, so known-vulnerable binaries are not left up for
download. Their source stays in git history.

## Security

Report vulnerabilities privately; see [SECURITY.md](SECURITY.md).
