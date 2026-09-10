#!/usr/bin/env python3
"""Assert every branding reference in the frontends resolves to a real file.

# What this catches that a build does not

Trunk fails a build on a missing ``data-trunk`` asset, so the two frontends'
*directories* are verified by building them. It does not check the individual
``href``/``src`` paths inside the HTML — a favicon link pointing at
``favicon-64x64.png``, which the pack does not ship, builds green and 404s in
the browser.

So this walks the built ``dist/`` of each frontend and resolves every reference
against the files actually shipped there.

It also scans each frontend's Rust source. The ``<head>`` links live in
``index.html``, but the header logo is rendered by wasm at runtime -- its path is
a string literal in a ``view!`` macro that never appears in the built HTML. A
check that only read ``index.html`` would report a green build for a page whose
logo is a broken image, which is precisely the case worth catching.

The explorer is not checked here: its HTML is a Rust string with no build step
to walk, and ``explorer/tests/server_tests.rs`` covers it by asking the running
server for every path the rendered page mentions. That is the stronger check of
the two, since it exercises the routes as well as the files.

Usage::

    python scripts/check_brand_refs.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Each frontend: its built output, and the source whose `view!` macros name
# assets that never reach `index.html`.
BUNDLES = [
    (ROOT / "dashboard" / "dist", ROOT / "dashboard" / "src"),
    (ROOT / "wallet-gui" / "ui" / "dist", ROOT / "wallet-gui" / "ui" / "src"),
]

# `src="assets/logo.png"` and the entries of a `srcset`, as written in Rust.
RUST_REFERENCE = re.compile(r'"((?:\./)?assets/[^"]+)"')

# Attribute values that name a file. `srcset` is deliberately included and
# handled specially below.
REFERENCE = re.compile(r'(?:href|src|srcset|content)="([^"]+)"')


def paths_in(value: str) -> list[str]:
    """Every file path inside one attribute value.

    A ``srcset`` is a comma-separated list of ``url descriptor`` pairs rather
    than a single URL, so each value is split on commas and the descriptor
    dropped. Treating a srcset as one path is a mistake that reports a
    perfectly good page as broken.
    """
    out = []
    for candidate in value.split(","):
        path = candidate.strip().split()[0] if candidate.strip() else ""
        if path:
            out.append(path)
    return out


def check(bundle: Path, source: Path) -> list[str]:
    """Unresolvable references in one built bundle and its source."""
    index = bundle / "index.html"
    if not index.exists():
        return [f"{bundle.relative_to(ROOT)} has no index.html — build it first"]

    values = REFERENCE.findall(index.read_text(encoding="utf-8"))

    # Every asset path written in a `view!` macro, from every Rust file in the
    # crate rather than a named one -- a logo moved to a new component should
    # still be checked.
    for rust in sorted(source.rglob("*.rs")):
        values += RUST_REFERENCE.findall(rust.read_text(encoding="utf-8"))

    problems = []
    for value in values:
        for path in paths_in(value):
            # External URLs, data URIs, and in-page anchors name no file.
            if path.startswith(("http://", "https://", "data:", "#", "//")):
                continue
            # Non-path `content=` values: a theme colour, a description.
            if "/" not in path and "." not in path:
                continue
            target = (bundle / path.lstrip("/")).resolve()
            if not target.exists():
                problems.append(f"{bundle.relative_to(ROOT)}: {path} does not exist")
    return problems


def main() -> int:
    problems = []
    for bundle, source in BUNDLES:
        found = check(bundle, source)
        problems += found
        if not found:
            print(f"ok  {bundle.relative_to(ROOT)} + {source.relative_to(ROOT)}")

    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1

    print(f"every branding reference in {len(BUNDLES)} bundles resolves")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
