#!/usr/bin/env -S uv run -q --with playwright==1.55.0 python3
"""Screenshots of the clickable prototype, one 240x320 PNG per state, plus the text layout of each state (position,
size, weight, colour of every text, in `layout.json`): the reference the firmware screens (crates/ui) are compared
with, see tools/compare_screens.py.

The prototype is set like the firmware today: no route, no sensor (`--route`, `--sensors` turn them on).

usage: tools/prototype_shots.py OUT_DIR [--route] [--sensors]   (CHROMIUM=/usr/bin/google-chrome if Playwright has
                                                                 no browser of its own)
"""
import json
import os
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
PAGE = ROOT / "docs/prototype/index.html"
SCREEN = '#app div[style*="width: 240px; height: 320px"]'
LAYOUT = """(scr) => {
  const r0 = scr.getBoundingClientRect(), out = [];
  const w = document.createTreeWalker(scr, NodeFilter.SHOW_TEXT);
  let n;
  while ((n = w.nextNode())) {
    const t = n.textContent.trim();
    if (!t) continue;
    const r = document.createRange(); r.selectNodeContents(n);
    const b = r.getBoundingClientRect();
    if (!b.width) continue;
    const cs = getComputedStyle(n.parentElement);
    out.push({ text: t, x: Math.round(b.x - r0.x), y: Math.round(b.y - r0.y), w: Math.round(b.width),
               h: Math.round(b.height), size: cs.fontSize, weight: cs.fontWeight, color: cs.color,
               spacing: cs.letterSpacing, upper: cs.textTransform === 'uppercase' });
  }
  return out;
}"""


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(executable_path=os.environ.get("CHROMIUM") or None)
        p = browser.new_page(viewport={"width": 900, "height": 900})
        p.goto(f"file://{PAGE}")
        p.wait_for_timeout(1500)

        def key(which: str):
            p.locator(f'#app [aria-label="Bouton {which}"]').click()
            p.wait_for_timeout(150)

        def sim(text: str):
            p.locator("#app button", has_text=text).first.click()
            p.wait_for_timeout(150)

        if "--route" not in sys.argv:
            sim("Trace chargée")
        if "--sensors" not in sys.argv:
            for s in ("Capteur de puissance", "Ceinture cardio", "Capteur de cadence"):
                sim(s)
        layouts = {}

        def shot(name: str):
            scr = p.locator(SCREEN).first
            scr.screenshot(path=str(out / f"{name}.png"))
            layouts[name] = scr.evaluate(LAYOUT)

        shot("home-search")
        p.wait_for_timeout(6200)
        shot("home")
        key("bas gauche")
        shot("menu")
        key("haut gauche")
        key("bas droite")
        # without route nor sensor: Essential, Altitude
        pages = 4 if "--route" in sys.argv else 2
        for i in range(pages):
            shot(f"ride-page{i + 1}")
            key("bas gauche")
        sim("Tunnel")
        shot("ride-gps-lost")
        sim("Sortir du tunnel")
        p.wait_for_timeout(3500)  # the "GPS retrouvé" banner
        key("bas droite")
        shot("paused")
        key("haut gauche")
        shot("pause-menu")
        key("haut gauche")
        key("bas gauche")
        shot("confirm-stop")
        key("bas droite")
        shot("saved")
        browser.close()
    (out / "layout.json").write_text(json.dumps(layouts, ensure_ascii=False, indent=1))
    print(f"{len(layouts)} states in {out}")


if __name__ == "__main__":
    main()
