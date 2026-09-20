# apps/dev-hub

Empty. No developer hub exists in this tree.

The foundation brief lists `dev-hub` under `apps/`, and also says not to build
product features in this phase. This directory is the reserved place.

The material a developer hub would present already exists and is generated,
which is the useful thing to know before writing one:

| Source | What it produces |
|---|---|
| `bins/docgen` | the LaTeX technical reference, from module documentation |
| `docs/site/` | the Astro documentation site |
| `crates/api-gateway` | an OpenAPI description, pinned by its own tests |
| `features.toml` | what exists and whether it works, machine-readable |

A hub built over those stays true as the code changes. One that restates them
by hand does not.
