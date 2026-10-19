#!/usr/bin/env -S uv run -q --with pillow==11.3.0 python3
"""Side by side, for every state of the same name: the prototype (tools/prototype_shots.py) and the firmware's
screen (crates/ui test, `cargo test -p ui`), each pair prototype | firmware, scaled up.

usage: tools/compare_screens.py OUT.png [PROTO_DIR] [FIRMWARE_DIR] [--names a,b,...] [--scale N]
       defaults: target/proto, target/tmp/screens, every common name, scale 2
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    opts = dict(a[2:].split("=", 1) for a in sys.argv[1:] if a.startswith("--") and "=" in a)
    out = Path(args[0])
    proto = Path(args[1]) if len(args) > 1 else ROOT / "target/proto"
    fw = Path(args[2]) if len(args) > 2 else ROOT / "target/tmp/screens"
    scale = int(opts.get("scale", 2))
    names = opts["names"].split(",") if "names" in opts else sorted(
        p.stem for p in proto.glob("*.png") if (fw / p.name).exists())
    if not names:
        sys.exit("no state in common")
    cols = min(3, len(names))
    rows = (len(names) + cols - 1) // cols
    pw, ph, gap, label = 240 * 2 + 8, 320, 16, 18
    sheet = Image.new("RGB", (cols * (pw + gap), rows * (ph + label + gap)), (128, 128, 128))
    draw = ImageDraw.Draw(sheet)
    for i, n in enumerate(names):
        x, y = (i % cols) * (pw + gap), (i // cols) * (ph + label + gap)
        draw.text((x + 2, y + 2), n, fill=(255, 255, 255))
        for j, d in enumerate((proto, fw)):
            im = Image.open(d / f"{n}.png").convert("RGB").crop((0, 0, 240, 320))
            sheet.paste(im, (x + j * 248, y + label))
    sheet = sheet.resize((sheet.width * scale // 2, sheet.height * scale // 2), Image.Resampling.NEAREST)
    sheet.save(out)
    print(f"{len(names)} states: {out}")


if __name__ == "__main__":
    main()
