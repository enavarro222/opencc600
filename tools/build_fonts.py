#!/usr/bin/env -S uv run -q --with freetype-py==2.5.1 --with uharfbuzz==0.51.1 --with pillow==11.3.0 python3
"""Rasterizes Barlow Condensed (crates/ui/fonts, SIL OFL 1.1) into the anti-aliased bitmap fonts of crates/ui.

Writes crates/ui/src/fonts/generated.rs (metrics, kerning) and crates/ui/src/fonts/*.bin (4-bit alpha, two pixels a
byte, rows packed). Sizes and weights are the prototype's (docs/prototype); figures are tabular (`tnum`), like the
prototype's values. Kerning comes from HarfBuzz (GPOS), in 1/64 px.

Every font also gets the UI icons (play, pause, power...) as glyphs in the Unicode private use area, drawn 8x larger
and scaled down: they sit on the baseline at the height of the capitals, so a label and its icon line up by
construction (`ICONS` below, same order as `crate::fonts::icon`).

usage: tools/build_fonts.py
"""
from pathlib import Path

import freetype
import uharfbuzz as hb
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT / "crates/ui/fonts"
OUT = ROOT / "crates/ui/src/fonts"

DIGITS = "0123456789,.:-+–% "
UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZÀÂÇÉÈÊËÎÏÔÙÛÜŒ"
FRENCH = "àâäçéèêëîïôöùûüÿœæÀÂÇÉÈÊËÎÏÔÖÙÛÜŒ«»’…·°−–›‹—"
TEXT = "".join(chr(c) for c in range(32, 127)) + FRENCH
LABEL = UPPER + DIGITS + "/·'’()%"

# private use code points of the icons, in the order of `crate::fonts::icon`
ICONS = ["play", "pause", "stop", "left", "right", "power", "check", "cross", "dot", "menu", "down", "chevron", "cycle",
         "undo", "ring", "zoom", "off_route"]
ICON_BASE = 0xE000
ICON_CHARS = "".join(chr(ICON_BASE + i) for i in range(len(ICONS)))

# name, weight, CSS px, characters
SPECS = [
    ("HUGE", "Bold", 84, DIGITS),
    ("ALT", "Bold", 78, DIGITS),
    ("VALUE", "Bold", 34, DIGITS),
    ("TITLE", "Bold", 30, TEXT),
    ("STAT", "Bold", 26, TEXT),
    ("HEADING", "Bold", 25, TEXT),
    ("HEADING_SMALL", "Bold", 22, TEXT),
    ("MENU", "SemiBold", 21, TEXT),
    ("BANNER", "Bold", 19, TEXT),
    ("KEY", "Bold", 18, TEXT),
    ("BODY_BOLD", "Bold", 18, TEXT),
    ("ROW", "SemiBold", 18, TEXT),
    ("ROW_SMALL", "SemiBold", 17, TEXT),
    ("BODY", "Medium", 18, TEXT),
    ("NOTE_LARGE", "Medium", 16, TEXT),
    ("SUB", "SemiBold", 15, TEXT),
    ("NOTE", "Medium", 15, TEXT),
    ("NOTE_BOLD", "Bold", 15, TEXT),
    ("BAR", "SemiBold", 14, TEXT),
    ("LABEL", "SemiBold", 13, LABEL),
    ("LABEL_SMALL", "SemiBold", 12, LABEL),
    ("LABEL_TINY", "SemiBold", 11, LABEL),
    ("MARK", "Bold", 44, ""),
]
# icon width, in cap heights
ICON_W = {"play": 0.9, "pause": 0.95, "stop": 0.9, "left": 0.9, "right": 0.9, "power": 1.08, "check": 1.1, "cross": 0.85,
          "dot": 0.62, "menu": 1.0, "down": 0.95, "chevron": 0.5, "cycle": 1.05, "undo": 1.05, "ring": 0.85, "zoom": 1.0,
          "off_route": 1.2}
SS = 8  # supersampling of the icons


def icon(name: str, cap: int, bold: bool) -> tuple[int, list[int]]:
    """Width and 0..255 alpha rows (cap rows) of an icon."""
    w = max(1, round(ICON_W[name] * cap))
    W, H = w * SS, cap * SS
    im = Image.new("L", (W, H), 0)
    dr = ImageDraw.Draw(im)
    t = max(1.8, cap * (0.19 if bold else 0.15)) * SS  # stroke
    if name in ("play", "right"):
        dr.polygon([(0, 0), (W, H / 2), (0, H)], fill=255)
    elif name == "left":
        dr.polygon([(W, 0), (0, H / 2), (W, H)], fill=255)
    elif name == "pause":
        bar = W * 0.37
        dr.rectangle([0, 0, bar, H], fill=255)
        dr.rectangle([W - bar, 0, W, H], fill=255)
    elif name == "stop":
        side = min(W, H)
        dr.rectangle([0, (H - side) / 2, side, (H + side) / 2], fill=255)
    elif name == "power":
        r = W / 2 - t / 2
        c = (W / 2, H / 2 + t * 0.15)
        dr.arc([c[0] - r, c[1] - r, c[0] + r, c[1] + r], -55, 235, fill=255, width=round(t))
        dr.line([(W / 2, 0), (W / 2, c[1])], fill=255, width=round(t))
    elif name == "check":
        dr.line([(t / 2, H * 0.55), (W * 0.38, H - t / 2), (W - t / 2, t / 2)], fill=255, width=round(t * 1.15),
                joint="curve")
    elif name == "cross":
        dr.line([(t / 2, t / 2), (W - t / 2, H - t / 2)], fill=255, width=round(t))
        dr.line([(W - t / 2, t / 2), (t / 2, H - t / 2)], fill=255, width=round(t))
    elif name == "dot":
        dr.ellipse([0, (H - W) / 2, W, (H + W) / 2], fill=255)
    elif name == "menu":
        for k in range(3):
            y = t / 2 + k * (H - t) / 2
            dr.line([(0, y), (W, y)], fill=255, width=round(t))
    elif name == "down":
        dr.polygon([(0, H * 0.18), (W, H * 0.18), (W / 2, H * 0.92)], fill=255)
    elif name == "chevron":
        dr.line([(t / 2, t / 2), (W - t / 2, H / 2), (t / 2, H - t / 2)], fill=255, width=round(t), joint="curve")
    elif name in ("cycle", "undo"):
        # a circle open at the top right, the arrow head at the top pointing clockwise (cycle); mirrored (undo)
        u = t * 0.72
        r = min(W, H) / 2 - u / 2
        c = (W / 2, H / 2)
        dr.arc([c[0] - r, c[1] - r, c[0] + r, c[1] + r], -20, 262, fill=255, width=round(u))
        top = (c[0], c[1] - r)
        h = u * 1.45
        dr.polygon([(top[0] + h * 1.25, top[1]), (top[0] - h * 0.25, top[1] - h), (top[0] - h * 0.25, top[1] + h)],
                   fill=255)
        if name == "undo":
            im = im.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
            dr = ImageDraw.Draw(im)
    elif name == "zoom":
        u = t * 0.75
        r = W / 2 - u / 2
        c = (W / 2, H / 2)
        dr.ellipse([c[0] - r, c[1] - r, c[0] + r, c[1] + r], outline=255, width=round(u))
        k = r * 0.55
        dr.line([(c[0] - k, c[1]), (c[0] + k, c[1])], fill=255, width=round(u))
        dr.line([(c[0], c[1] - k), (c[0], c[1] + k)], fill=255, width=round(u))
    elif name == "off_route":
        # the route, a line on the left; the rider, a dot on the right; dots between them
        u = t * 0.8
        dr.rectangle([0, 0, u, H], fill=255)
        r = H * 0.25
        c = (W - r, H / 2)
        dr.ellipse([c[0] - r, c[1] - r, c[0] + r, c[1] + r], fill=255)
        for k in (1, 2):
            x = u + k * (c[0] - r - u) / 3
            dr.ellipse([x - u * 0.45, H / 2 - u * 0.45, x + u * 0.45, H / 2 + u * 0.45], fill=255)
    elif name == "ring":
        r = W / 2 - t * 0.35
        c = (W / 2, H / 2)
        dr.ellipse([c[0] - r, c[1] - r, c[0] + r, c[1] + r], outline=255, width=round(t * 0.7))
    small = im.resize((w, cap), Image.Resampling.BOX)
    return w, list(small.getdata())
FEATURES = {"kern": True, "tnum": True}
# smaller kerning is lost in the rounding of the glyph positions (1/64 px)
KERN_MIN = 16


def shape(font: hb.Font, text: str) -> list[tuple[int, int]]:
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(font, buf, FEATURES)
    return [(i.codepoint, p.x_advance) for i, p in zip(buf.glyph_infos, buf.glyph_positions)]


def build(name: str, weight: str, px: int, chars: str) -> tuple[str, bytes]:
    path = FONTS / f"BarlowCondensed-{weight}.ttf"
    face = freetype.Face(str(path))
    face.set_pixel_sizes(0, px)
    blob = hb.Blob.from_file_path(str(path))
    hbf = hb.Font(hb.Face(blob))
    hbf.scale = (px * 64, px * 64)  # advances in 1/64 px, like FreeType's 26.6

    chars = sorted(set(chars))
    glyphs, data, missing = [], bytearray(), []
    for c in chars:
        if face.get_char_index(c) == 0:
            missing.append(c)
            continue
        [(gid, adv)] = shape(hbf, c)  # the tabular glyph for figures
        face.load_glyph(gid, freetype.FT_LOAD_TARGET_LIGHT | freetype.FT_LOAD_RENDER)
        g = face.glyph
        bm = g.bitmap
        w, h = bm.width, bm.rows
        off = len(data)
        nibbles = []
        for y in range(h):
            row = bm.buffer[y * bm.pitch:y * bm.pitch + w]
            nibbles += [(v * 15 + 127) // 255 for v in row]
        if len(nibbles) % 2:
            nibbles.append(0)
        data += bytes(nibbles[i] << 4 | nibbles[i + 1] for i in range(0, len(nibbles), 2))
        glyphs.append((c, adv, g.bitmap_left, g.bitmap_top, w, h, off))
    if missing:
        print(f"{name}: not in Barlow Condensed: {''.join(missing)!r}")
    face.load_char("H", freetype.FT_LOAD_TARGET_LIGHT | freetype.FT_LOAD_RENDER)
    cap = face.glyph.bitmap_top
    for i, icon_name in enumerate(ICONS):
        w, alpha = icon(icon_name, cap, weight == "Bold")
        nibbles = [(v * 15 + 127) // 255 for v in alpha]
        if len(nibbles) % 2:
            nibbles.append(0)
        off = len(data)
        data += bytes(nibbles[j] << 4 | nibbles[j + 1] for j in range(0, len(nibbles), 2))
        glyphs.append((chr(ICON_BASE + i), (w + max(1, round(px * 0.06))) * 64, 0, cap, w, cap, off))

    # pair kerning: what shaping the pair adds to the two advances
    present = [g[0] for g in glyphs if g[0] not in ICON_CHARS]
    alone = {c: shape(hbf, c)[0][1] for c in present}
    kern = []
    for a in present:
        for b in present:
            k = sum(x for _, x in shape(hbf, a + b)) - alone[a] - alone[b]
            if abs(k) >= KERN_MIN:
                kern.append((a, b, k))

    m = face.size
    file = f"{name.lower()}.bin"
    (OUT / file).write_bytes(bytes(data))
    rs = [f"/// Barlow Condensed {weight}, {px} px: {len(glyphs)} glyphs, {len(kern)} kerning pairs, {len(data)} bytes."]
    rs.append(f"pub static {name}: Font = Font {{")
    rs.append(f"    px: {px},")
    rs.append(f"    ascent: {m.ascender // 64},")
    rs.append(f"    descent: {-m.descender // 64},")
    rs.append(f"    cap: {cap},")
    rs.append("    glyphs: &[")
    for c, adv, x, y, w, h, off in glyphs:
        rs.append(f"        Glyph {{ c: {rust_char(c)}, adv: {adv}, x: {x}, y: {y}, w: {w}, h: {h}, off: {off} }},")
    rs.append("    ],")
    rs.append("    kerning: &[")
    for a, b, k in kern:
        rs.append(f"        ({rust_char(a)}, {rust_char(b)}, {k}),")
    rs.append("    ],")
    rs.append(f'    bitmaps: include_bytes!("{file}"),')
    rs.append("};")
    return "\n".join(rs), bytes(data)


def rust_char(c: str) -> str:
    if ord(c) >= ICON_BASE:
        return f"'\\u{{{ord(c):x}}}'"
    return {"'": "'\\''", "\\": "'\\\\'"}.get(c, f"'{c}'")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    parts = [
        "//! Generated by tools/build_fonts.py from Barlow Condensed, Copyright 2017 The Barlow Project Authors",
        "//! (https://github.com/jpt/barlow), SIL Open Font License 1.1 (crates/ui/fonts/OFL.txt). Do not edit.",
        "//! The font data (this file and the .bin files next to it) stays under the OFL, not the GPL.",
        "",
        "use super::{Font, Glyph};",
    ]
    total = 0
    for spec in SPECS:
        rs, data = build(*spec)
        parts += ["", rs]
        total += len(data)
    (OUT / "generated.rs").write_text("\n".join(parts) + "\n")
    print(f"{len(SPECS)} fonts, {total} bytes of bitmaps")


if __name__ == "__main__":
    main()
