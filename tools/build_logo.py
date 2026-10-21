#!/usr/bin/env -S uv run -q --with fonttools==4.62.1 --with uharfbuzz==0.51.1 python3
"""Draws the splash screen's logo (crates/ui/src/draw.rs, `splash`) as SVG: docs/logo.svg (day theme) and
docs/logo-night.svg (night theme), and the same with the text on the right, docs/logo-wide.svg and
docs/logo-wide-night.svg. Transparent background, the text as paths (Barlow Condensed, shaped by HarfBuzz) so that it
needs no font where it is shown.

Same geometry as the firmware, in the screen's pixels, without the version.
"""
import math
from pathlib import Path

import uharfbuzz as hb
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT / "crates/ui/fonts"

# bg, fg, mute, route of draw.rs's DAY and NIGHT
THEMES = {
    "": ("#FFFFFF", "#111111", "#555555", "#D9480F"),
    "-night": ("#0B0B0C", "#F2F2F2", "#A3A3A3", "#FF7A45"),
}
CX, CY = 120.0, 116.0


def mix(a, b):
    return "#" + "".join(f"{(int(a[i:i + 2], 16) + int(b[i:i + 2], 16)) // 2:02X}" for i in (1, 3, 5))


def f(v):
    return f"{v:.2f}".rstrip("0").rstrip(".")


def circle(x, y, r):
    """A circle as a closed subpath, to combine with even-odd fills."""
    return f"M{f(x - r)} {f(y)}a{f(r)} {f(r)} 0 1 0 {f(2 * r)} 0a{f(r)} {f(r)} 0 1 0 {f(-2 * r)} 0z"


def text(s, weight, px, cy, x=None, xc=None):
    """`s` as one SVG path, its capitals centred on `cy` like the firmware's text (so that pieces share a baseline),
    its ink centred on `xc` or starting at `x`; returns (d, advance width)."""
    path = FONTS / f"BarlowCondensed-{weight}.ttf"
    tt = TTFont(path)
    glyphs = tt.getGlyphSet()
    upem = tt["head"].unitsPerEm
    face = hb.Face(hb.Blob.from_file_path(str(path)))
    font = hb.Font(face)
    buf = hb.Buffer()
    buf.add_str(s)
    buf.guess_segment_properties()
    hb.shape(font, buf, {"kern": True})
    order = tt.getGlyphOrder()
    placed, pen_x = [], 0
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        placed.append((order[info.codepoint], pen_x + pos.x_offset, pos.y_offset))
        pen_x += pos.x_advance
    # ink box, in font units
    xs = []
    for name, gx, gy in placed:
        bp = BoundsPen(glyphs)
        glyphs[name].draw(bp)
        if bp.bounds:
            xs += [gx + bp.bounds[0], gx + bp.bounds[2]]
    k = px / upem
    width = (max(xs) - min(xs)) * k
    left = (xc - width / 2) if xc is not None else x
    ox = left - min(xs) * k
    oy = cy + tt["OS/2"].sCapHeight * k / 2
    svg = SVGPathPen(glyphs, ntos=f)
    for name, gx, gy in placed:
        glyphs[name].draw(TransformPen(svg, (k, 0, 0, -k, ox + gx * k, oy - gy * k)))
    return svg.getCommands(), pen_x * k


def gear(c, fg, cx, cy):
    """The freewheel sprocket with the route through its bore, centred on (cx, cy)."""
    body = mix(c, fg)
    teeth, root, tip = 20, 47.0, 56.0
    step = math.tau / teeth
    seat_r, seat = root + 1.5, 4.2

    def at(r, a):
        return cx + r * math.cos(a), cy + r * math.sin(a)

    # the cuts between the teeth: the round seats and the flanks opening outwards
    cuts = []
    for k in range(teeth):
        a = (k + 0.5) * step
        cuts.append(f'<circle cx="{f(at(seat_r, a)[0])}" cy="{f(at(seat_r, a)[1])}" r="{seat}"/>')
        h0, h1 = seat / seat_r, 9.4 / (tip + 4.0)
        pts = [at(seat_r, a - h0), at(seat_r, a + h0), at(tip + 4.0, a + h1), at(tip + 4.0, a - h1)]
        cuts.append(f'<polygon points="{" ".join(f"{f(x)},{f(y)}" for x, y in pts)}"/>')

    # the route through the bore: an S from a ringed start to the arrival
    def route(u):
        return (cx - 13.0 + 26.0 * u + 5.0 * math.sin(math.tau * u), cy + 11.0 * math.cos(math.pi * u))

    line = " ".join(f"{f(x)},{f(y)}" for x, y in (route(i / 96) for i in range(97)))
    (sx, sy), (ex, ey) = route(0.0), route(1.0)
    return f"""  <defs>
    <mask id="teeth">
      <circle cx="{f(cx)}" cy="{f(cy)}" r="{tip}" fill="#fff"/>
      <g fill="#000">
        {chr(10).join("        " + s for s in cuts).lstrip()}
        <circle cx="{f(cx)}" cy="{f(cy)}" r="38"/>
      </g>
    </mask>
  </defs>
  <circle cx="{f(cx)}" cy="{f(cy)}" r="{tip}" fill="{c}" mask="url(#teeth)"/>
  <path fill="{body}" fill-rule="evenodd" d="{circle(cx, cy, 36.8)}{circle(cx, cy, 27.5)}{circle(cx - 32.2, cy, 2.3)}{circle(cx + 32.2, cy, 2.3)}"/>
  <path fill="{c}" fill-rule="evenodd" d="{circle(cx, cy, 27.5)}{circle(cx, cy, 24)}"/>
  <polyline points="{line}" fill="none" stroke="{fg}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
  <path fill="{fg}" fill-rule="evenodd" d="{circle(sx, sy, 4)}{circle(sx, sy, 2)}"/>
  <circle cx="{f(ex)}" cy="{f(ey)}" r="4.2" fill="{c}"/>
"""


def name(fg, mute, px, cy, x=None, xc=None):
    """Open in the mute colour, CC600 strong, as one word; returns (svg, advance width)."""
    _, wo = text("Open", "Bold", px, cy, x=0)
    _, wc = text("CC600", "Bold", px, cy, x=0)
    x0 = x if x is not None else xc - (wo + wc) / 2
    open_d, _ = text("Open", "Bold", px, cy, x=x0)
    cc_d, _ = text("CC600", "Bold", px, cy, x=x0 + wo)
    return f'  <path fill="{mute}" d="{open_d}"/>\n  <path fill="{fg}" d="{cc_d}"/>\n', wo + wc


def svg(view, scale, body):
    w, h = view[2] * scale, view[3] * scale
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="{" ".join(f(v) for v in view)}" width="{f(w)}" height="{f(h)}">
  <title>OpenCC600</title>
{body}</svg>
"""


def stacked(bg, fg, mute, c):
    """As on the splash screen: the sprocket, the name and the motto under it."""
    motto_d, _ = text("Rouler en liberté", "Medium", 18, 246, xc=CX)
    return svg((40, 52, 160, 206), 2, gear(c, fg, CX, CY) + name(fg, mute, 30, 214, xc=CX)[0]
               + f'  <path fill="{mute}" d="{motto_d}"/>\n')


def wide(bg, fg, mute, c):
    """The sprocket on the left, the name and the motto on its right."""
    x = 132
    name_svg, w = name(fg, mute, 46, 46, x=x)
    motto_d, wm = text("Rouler en liberté", "Medium", 24, 84, x=x + 1)
    right = x + max(w, wm + 1) + 4
    return svg((0, 0, right, 120), 2, gear(c, fg, 60, 60) + name_svg + f'  <path fill="{mute}" d="{motto_d}"/>\n')


if __name__ == "__main__":
    for layout, draw in (("logo", stacked), ("logo-wide", wide)):
        for suffix, theme in THEMES.items():
            out = ROOT / "docs" / f"{layout}{suffix}.svg"
            out.write_text(draw(*theme))
            print(out.relative_to(ROOT))
