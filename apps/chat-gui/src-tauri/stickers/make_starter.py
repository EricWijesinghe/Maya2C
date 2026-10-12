"""Draws the starter sticker pack: twelve animated SVGs of one character.

Run from this directory: `python make_starter.py`. It writes starter/*.svg,
which the app embeds (`include_bytes!` in src/starter.rs). Original art in the
Forged Light palette (teal, bronze, steel); SMIL animation, so it moves inside
an <img>, where no script can run. Each file stays well under 8 KB.
"""
from pathlib import Path

TEAL, TEAL_DEEP, BRONZE, INK, WHITE, BLUSH = "#1fb5c2", "#0d6f78", "#c9a86a", "#0a161b", "#f4fbfb", "#f08a8a"

HEAD = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
<defs><radialGradient id="g" cx="38%" cy="32%" r="70%"><stop offset="0" stop-color="#7eeaf0"/><stop offset=".55" stop-color="{TEAL}"/><stop offset="1" stop-color="{TEAL_DEEP}"/></radialGradient></defs>"""


def body(squash="1 1;1.04 .96;1 1", dur="1.6s", extra=""):
    """The orb, breathing. `extra` is drawn inside the same moving group."""
    return f"""<g transform="translate(256 286)"><g>
<animateTransform attributeName="transform" type="scale" values="{squash}" dur="{dur}" repeatCount="indefinite"/>
<ellipse cx="0" cy="168" rx="120" ry="16" fill="{INK}" opacity=".18"/>
<circle r="170" fill="url(#g)" stroke="{INK}" stroke-width="10"/>
<ellipse cx="-62" cy="-92" rx="46" ry="26" fill="{WHITE}" opacity=".45" transform="rotate(-28 -62 -92)"/>
{extra}</g></g>"""


def eyes(kind="open"):
    if kind == "open":
        blink = f'<animate attributeName="ry" values="22;22;2;22" keyTimes="0;.9;.95;1" dur="3s" repeatCount="indefinite"/>'
        return (f'<ellipse cx="-56" cy="-20" rx="18" ry="22" fill="{INK}">{blink}</ellipse>'
                f'<ellipse cx="56" cy="-20" rx="18" ry="22" fill="{INK}">{blink}</ellipse>')
    if kind == "happy":
        return (f'<path d="M-80 -18 q24 -28 48 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
                f'<path d="M32 -18 q24 -28 48 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>')
    if kind == "closed":
        return (f'<path d="M-80 -16 q24 18 48 0" fill="none" stroke="{INK}" stroke-width="10" stroke-linecap="round"/>'
                f'<path d="M32 -16 q24 18 48 0" fill="none" stroke="{INK}" stroke-width="10" stroke-linecap="round"/>')
    if kind == "wide":
        return (f'<circle cx="-56" cy="-24" r="28" fill="{WHITE}" stroke="{INK}" stroke-width="8"/><circle cx="-56" cy="-22" r="12" fill="{INK}"/>'
                f'<circle cx="56" cy="-24" r="28" fill="{WHITE}" stroke="{INK}" stroke-width="8"/><circle cx="56" cy="-22" r="12" fill="{INK}"/>')
    if kind == "angry":
        return (f'<path d="M-90 -58 l60 22" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
                f'<path d="M90 -58 l-60 22" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
                f'<ellipse cx="-56" cy="-14" rx="15" ry="17" fill="{INK}"/><ellipse cx="56" cy="-14" rx="15" ry="17" fill="{INK}"/>')
    raise ValueError(kind)


def heart(x, y, s, delay):
    return (f'<g transform="translate({x} {y}) scale({s})"><path d="M0 12 C-20 -8 -40 4 -28 22 L0 46 L28 22 C40 4 20 -8 0 12Z" fill="{BLUSH}" stroke="{INK}" stroke-width="5">'
            f'<animateTransform attributeName="transform" type="scale" values="1;1.25;1" dur=".8s" begin="{delay}" repeatCount="indefinite"/></path></g>')


def cheeks():
    return (f'<ellipse cx="-96" cy="30" rx="22" ry="12" fill="{BLUSH}" opacity=".7"/>'
            f'<ellipse cx="96" cy="30" rx="22" ry="12" fill="{BLUSH}" opacity=".7"/>')


STICKERS = {
    "01-happy": ("😊", body(extra=eyes("happy") + cheeks() +
        f'<path d="M-52 40 q52 50 104 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>')),
    "02-laugh": ("😂", body("1 1;1.08 .92;.96 1.05;1 1", ".5s", eyes("happy") +
        f'<path d="M-66 28 h132 q-10 76 -66 76 q-56 0 -66 -76z" fill="{INK}"/><path d="M-40 70 q40 30 80 0 v20 q-40 20 -80 0z" fill="{BLUSH}"/>'
        f'<path d="M-100 -4 q-14 30 -2 46" fill="none" stroke="#7eeaf0" stroke-width="10" stroke-linecap="round"><animate attributeName="opacity" values="1;.2;1" dur=".5s" repeatCount="indefinite"/></path>'
        f'<path d="M100 -4 q14 30 2 46" fill="none" stroke="#7eeaf0" stroke-width="10" stroke-linecap="round"><animate attributeName="opacity" values=".2;1;.2" dur=".5s" repeatCount="indefinite"/></path>')),
    "03-love": ("😍", body(extra=heart(-56, -48, 1.1, "0s") + heart(56, -48, 1.1, ".4s") + cheeks() +
        f'<path d="M-44 44 q44 40 88 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>')),
    "04-sad": ("😢", body("1 1;1.01 .99;1 1", "2.4s", eyes("open") +
        f'<path d="M-44 70 q44 -36 88 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
        f'<path d="M66 4 q12 22 0 34 q-12 -12 0 -34z" fill="#7eeaf0" stroke="{INK}" stroke-width="4"><animateTransform attributeName="transform" type="translate" values="0 0;0 60;0 60" keyTimes="0;.8;1" dur="1.8s" repeatCount="indefinite"/><animate attributeName="opacity" values="1;1;0" keyTimes="0;.8;1" dur="1.8s" repeatCount="indefinite"/></path>')),
    "05-angry": ("😠", body("1 1;1.03 .97;.97 1.03;1 1", ".35s", eyes("angry") +
        f'<path d="M-44 66 q44 -30 88 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
        f'<g fill="none" stroke="{BRONZE}" stroke-width="10" stroke-linecap="round" transform="translate(120 -130)"><path d="M0 0 l14 14 M28 0 l-14 14 M0 28 l14 -14 M28 28 l-14 -14"><animate attributeName="opacity" values="1;.3;1" dur=".6s" repeatCount="indefinite"/></path></g>')),
    "06-wow": ("😮", body(extra=eyes("wide") +
        f'<ellipse cx="0" cy="64" rx="26" ry="30" fill="{INK}"><animate attributeName="ry" values="30;38;30" dur="1s" repeatCount="indefinite"/></ellipse>')),
    "07-thinking": ("🤔", body("1 1;1 1", "3s", eyes("open") +
        f'<path d="M-30 64 h60" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
        f'<g transform="translate(150 -150)"><circle r="12" fill="{WHITE}" stroke="{INK}" stroke-width="5"><animate attributeName="opacity" values="0;1;1;0" keyTimes="0;.2;.8;1" dur="2.4s" repeatCount="indefinite"/></circle>'
        f'<circle cx="26" cy="-34" r="20" fill="{WHITE}" stroke="{INK}" stroke-width="5"><animate attributeName="opacity" values="0;0;1;1;0" keyTimes="0;.2;.4;.8;1" dur="2.4s" repeatCount="indefinite"/></circle>'
        f'<text x="30" y="-26" font-size="26" font-family="sans-serif" font-weight="700" text-anchor="middle" fill="{INK}">?<animate attributeName="opacity" values="0;0;1;1;0" keyTimes="0;.3;.45;.8;1" dur="2.4s" repeatCount="indefinite"/></text></g>')),
    "08-thumbs-up": ("👍", body(extra=eyes("happy") +
        f'<path d="M-40 46 q40 32 80 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
        f'<g transform="translate(150 60) rotate(-10)"><g><animateTransform attributeName="transform" type="rotate" values="0;-14;0" dur=".9s" repeatCount="indefinite"/>'
        f'<rect x="-30" y="-10" width="60" height="70" rx="16" fill="{BRONZE}" stroke="{INK}" stroke-width="8"/><rect x="-14" y="-62" width="28" height="60" rx="14" fill="{BRONZE}" stroke="{INK}" stroke-width="8"/></g></g>')),
    "09-party": ("🎉", body("1 1;1.06 .94;1 1", ".7s", eyes("happy") + cheeks() +
        f'<path d="M-52 36 q52 56 104 0z" fill="{INK}"/>'
        f'<path d="M-40 -150 l40 -110 l40 110z" fill="{BRONZE}" stroke="{INK}" stroke-width="8"/><circle cy="-262" r="14" fill="{BLUSH}" stroke="{INK}" stroke-width="5"/>'
        + "".join(f'<rect x="{x}" y="-220" width="14" height="22" rx="3" fill="{c}"><animateTransform attributeName="transform" type="translate" values="0 0;{dx} 380" dur="{d}s" begin="{b}s" repeatCount="indefinite"/><animate attributeName="opacity" values="1;0" dur="{d}s" begin="{b}s" repeatCount="indefinite"/></rect>'
                  for x, dx, c, d, b in [(-180, -40, BLUSH, 1.6, 0), (-120, 30, BRONZE, 1.4, .3), (130, 40, "#7eeaf0", 1.5, .1), (180, -30, BLUSH, 1.7, .6), (60, 20, BRONZE, 1.3, .9)]))),
    "10-sleepy": ("😴", body("1 1;1.05 .95;1 1", "3s", eyes("closed") +
        f'<ellipse cx="0" cy="60" rx="16" ry="10" fill="{INK}"/>'
        + "".join(f'<text x="{110 + 30 * i}" y="{-110 - 40 * i}" font-size="{36 + 10 * i}" font-family="sans-serif" font-weight="800" fill="{WHITE}" stroke="{INK}" stroke-width="3">z<animate attributeName="opacity" values="0;1;0" dur="2.4s" begin="{.6 * i}s" repeatCount="indefinite"/></text>' for i in range(3)))),
    "11-cool": ("😎", body("1 1;1.02 .98;1 1", "1.2s",
        f'<path d="M-112 -46 h224 v20 q0 50 -50 50 q-40 0 -48 -40 h-28 q-8 40 -48 40 q-50 0 -50 -50z" fill="{INK}"/>'
        f'<path d="M-84 -36 l30 0 l-40 34z" fill="{WHITE}" opacity=".5"><animate attributeName="opacity" values="0;.6;0" dur="2s" repeatCount="indefinite"/></path>'
        f'<path d="M-40 52 q50 26 90 -10" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>')),
    "12-wave": ("👋", body(extra=eyes("happy") + cheeks() +
        f'<path d="M-46 40 q46 46 92 0" fill="none" stroke="{INK}" stroke-width="12" stroke-linecap="round"/>'
        f'<g transform="translate(-170 40)"><g><animateTransform attributeName="transform" type="rotate" values="-20;25;-20" dur=".7s" repeatCount="indefinite"/>'
        f'<ellipse cx="0" cy="-56" rx="34" ry="44" fill="{BRONZE}" stroke="{INK}" stroke-width="8"/></g></g>')),
}


def main():
    out = Path(__file__).with_name("starter")
    out.mkdir(exist_ok=True)
    for name, (emoji, art) in STICKERS.items():
        svg = HEAD + art + "</svg>\n"
        assert len(svg.encode()) < 8 * 1024, name
        (out / f"{name}.svg").write_text(svg, encoding="utf-8", newline="\n")
        print(f"{name}.svg {emoji} {len(svg.encode())} B")


main()
