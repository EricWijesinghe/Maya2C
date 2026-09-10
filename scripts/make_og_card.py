#!/usr/bin/env python3
"""Compose the OpenGraph social card from the logo asset pack.

# Why this exists rather than a checked-in PNG with no recipe

The asset pack has no 1200x630 image. Its closest social banners are
``TwitterX-Banner_1500x500`` (3:1) and ``Facebook-Cover_820x312`` (2.63:1), and
neither is the 1.91:1 that scrapers crop to. Shipping one of those verbatim gets
letterboxed on some platforms and cropped on others.

So the card is generated -- and generated *by a script that is committed*, so the
next person can see exactly what was composed from what, and regenerate it when
the pack changes. A binary that appeared in a commit with no recipe is a binary
nobody can reproduce or correct.

# Every input comes from the pack

The wordmark is ``website/Header-Logo-2x_500x200.png`` verbatim. The background is
``#ffffff``, read from the pack's own ``favicons/site.webmanifest``
(``background_color``) rather than picked by eye -- if the brand background
changes, it changes there and this follows.

Nothing is redrawn, recoloured, or traced. The script scales and centres, and
that is all it does.

Usage::

    python scripts/make_og_card.py
    python scripts/make_og_card.py --check   # verify outputs are current
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PACK = ROOT / "logo-assets"

# The 1.91:1 that Facebook, LinkedIn, Slack and Twitter/X all crop toward.
# 1200x630 is the size every one of them documents.
CARD = (1200, 630)

# The wordmark occupies this fraction of the card's width. Two thirds leaves a
# margin wide enough that a platform cropping to a tighter ratio still contains
# the whole mark -- the point of a card is to survive somebody else's crop.
LOGO_WIDTH_FRACTION = 2 / 3

SOURCE_LOGO = PACK / "website" / "Header-Logo-2x_500x200.png"
MANIFEST = PACK / "favicons" / "site.webmanifest"

# Where the finished card is deployed. Both apps serve their own copy rather
# than sharing one, because they are separate deployments that may not sit on
# the same host.
DESTINATIONS = [
    ROOT / "explorer" / "assets" / "og-image.png",
    ROOT / "dashboard" / "assets" / "og-image.png",
]


def background_colour() -> str:
    """The pack's own declared background colour.

    Read rather than hardcoded: this is the same value the manifest hands the
    browser for the splash screen, and a card that disagreed with it would be a
    second opinion about the brand living in a script.
    """
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    colour = manifest.get("background_color")
    if not colour:
        raise SystemExit(f"{MANIFEST} declares no background_color")
    return colour


def compose() -> Image.Image:
    """The card: the wordmark, centred, on the pack's background."""
    card = Image.new("RGBA", CARD, background_colour())
    logo = Image.open(SOURCE_LOGO).convert("RGBA")

    target_width = int(CARD[0] * LOGO_WIDTH_FRACTION)
    scale = target_width / logo.width
    resized = logo.resize(
        (target_width, int(logo.height * scale)),
        # LANCZOS, because the source is larger than the target in both
        # dimensions and this is a downscale. A nearest-neighbour downscale of a
        # wordmark produces visible stair-stepping on the letterforms.
        Image.Resampling.LANCZOS,
    )

    card.paste(
        resized,
        ((CARD[0] - resized.width) // 2, (CARD[1] - resized.height) // 2),
        # The wordmark has an alpha channel; pasting without the mask would
        # fill its transparent regions with black.
        resized,
    )
    return card


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the deployed cards match what this script produces, and "
        "change nothing",
    )
    args = parser.parse_args()

    card = compose()

    if args.check:
        import io

        buffer = io.BytesIO()
        card.save(buffer, "PNG")
        expected = buffer.getvalue()

        stale = [d for d in DESTINATIONS if not d.exists() or d.read_bytes() != expected]
        for destination in stale:
            print(f"stale or missing: {destination.relative_to(ROOT)}", file=sys.stderr)
        if stale:
            print("run: python scripts/make_og_card.py", file=sys.stderr)
            return 1
        print(f"{len(DESTINATIONS)} cards current")
        return 0

    for destination in DESTINATIONS:
        destination.parent.mkdir(parents=True, exist_ok=True)
        card.save(destination, "PNG")
        print(f"wrote {destination.relative_to(ROOT)} ({CARD[0]}x{CARD[1]})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
