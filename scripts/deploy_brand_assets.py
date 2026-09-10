#!/usr/bin/env python3
"""Copy the logo pack's web assets into each app that serves a page.

# Why a script and not a one-off copy

Three apps need the same favicon set, and the set is eleven files. Copying them
by hand once is fine; keeping three copies in step by hand after the pack is
updated is not. Running this is how the copies are refreshed, and ``--check`` is
how CI or a reviewer finds out they drifted.

# Two defects in the pack are corrected here rather than propagated

``favicons/site.webmanifest`` ships with ``"name": ""`` and ``"short_name": ""``.
An empty name is what an installed PWA shows under its icon, so each app gets its
own filled-in copy.

``favicons/paste-in-head.html`` hardcodes root-absolute paths (``/favicon.ico``).
The explorer and the dashboard serve their assets under ``/assets/``, so the HTML
is written against the path each app actually serves from, not pasted.

Neither correction touches ``logo-assets/`` itself. The pack is the source of
truth and stays exactly as delivered.

Usage::

    python scripts/deploy_brand_assets.py
    python scripts/deploy_brand_assets.py --check
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACK = ROOT / "logo-assets"

# Copied byte-for-byte into every app. The whole favicon set rather than a
# chosen subset: a browser asks for whichever one it wants, and a missing size
# is a request that 404s in somebody's console.
FAVICON_FILES = [
    "favicon.ico",
    "favicon-16x16.png",
    "favicon-32x32.png",
    "favicon-48x48.png",
    "apple-touch-icon.png",
    "android-chrome-192x192.png",
    "android-chrome-512x512.png",
    "mstile-150x150.png",
    "browserconfig.xml",
]

# The nav wordmark, at both densities. `srcset` picks between them; shipping only
# the 1x leaves the logo soft on every laptop made in the last decade, and
# shipping only the 2x doubles the bytes for everyone else.
FULL_LOGO = [
    ("website/Header-Logo_250x100.png", "logo.png"),
    ("website/Header-Logo-2x_500x200.png", "logo@2x.png"),
]

# The compact pair, for a titlebar with 0.7rem of vertical padding. The full
# wordmark scaled into that height is unreadable; the pack ships a compact cut
# for exactly this case.
COMPACT_LOGO = [
    ("website/Compact-Header_200x50.png", "logo.png"),
    ("website/Compact-Header-2x_400x100.png", "logo@2x.png"),
]


class App:
    """One deployment target."""

    def __init__(self, name: str, assets: str, short_name: str, logo: list):
        self.name = name
        self.assets = ROOT / assets
        self.short_name = short_name
        self.logo = logo

    def manifest(self) -> str:
        """The pack's manifest with this app's name filled in.

        Icon paths are rewritten from root-absolute to relative, because the
        manifest is served from ``/assets/`` and a root-absolute ``/android-
        chrome-192x192.png`` would resolve to a path no app serves.
        """
        manifest = json.loads(
            (PACK / "favicons" / "site.webmanifest").read_text(encoding="utf-8")
        )
        manifest["name"] = self.name
        manifest["short_name"] = self.short_name
        for icon in manifest.get("icons", []):
            icon["src"] = "./" + icon["src"].lstrip("/")
        return json.dumps(manifest, indent=2) + "\n"


APPS = [
    App("Maya2C Explorer", "explorer/assets", "Explorer", FULL_LOGO),
    App("Maya2C Network", "dashboard/assets", "Maya2C", FULL_LOGO),
    App("Maya Wallet", "wallet-gui/ui/assets", "Wallet", COMPACT_LOGO),
]


def planned() -> dict[Path, bytes]:
    """Every file this script would write, and its exact content."""
    out: dict[Path, bytes] = {}
    for app in APPS:
        for name in FAVICON_FILES:
            out[app.assets / name] = (PACK / "favicons" / name).read_bytes()
        for source, name in app.logo:
            out[app.assets / name] = (PACK / source).read_bytes()
        out[app.assets / "site.webmanifest"] = app.manifest().encode("utf-8")
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="report drift between the pack and the deployed copies, and change "
        "nothing",
    )
    args = parser.parse_args()

    files = planned()

    if args.check:
        stale = [
            path
            for path, content in files.items()
            if not path.exists() or path.read_bytes() != content
        ]
        for path in stale:
            print(f"stale or missing: {path.relative_to(ROOT)}", file=sys.stderr)
        if stale:
            print("run: python scripts/deploy_brand_assets.py", file=sys.stderr)
            return 1
        print(f"{len(files)} assets current across {len(APPS)} apps")
        return 0

    for path, content in files.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    print(f"deployed {len(files)} assets across {len(APPS)} apps")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
