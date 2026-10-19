#!/usr/bin/env python3
"""Walk the clickable prototype through its main states in headless Chromium and check that the
key labels and key bar never change size; also report any text cut by overflow.

Usage: python3 tools/check_prototype_layout.py docs/prototype/index.html
Needs: pip install playwright && playwright install chromium (or set CHROMIUM to a Chromium executable).
"""
import os
import sys, json
from playwright.sync_api import sync_playwright
JS = """() => {
  const scr = document.querySelector('#app div[style*="width: 240px; height: 320px"]');
  const r0 = scr.getBoundingClientRect();
  const rect = el => { const r = el.getBoundingClientRect(); return [Math.round(r.x-r0.x), Math.round(r.y-r0.y), Math.round(r.width), Math.round(r.height)]; };
  const top = scr.querySelector('button');
  const bar = [...scr.querySelectorAll('div')].find(d => (d.getAttribute('style')||'').includes('32px minmax'));
  const right = top.nextElementSibling;
  const cut = [...scr.querySelectorAll('button, span')].filter(e => e.scrollWidth > e.clientWidth + 0.5 && getComputedStyle(e).overflow === 'hidden').map(e => e.textContent.trim());
  return { top: rect(top), bar: [...bar.children].map(rect), right: rect(right),
           rightCut: right.scrollWidth > right.clientWidth + 0.5, cut,
           labels: [...bar.children].map(e => e.textContent.trim()).concat([top.textContent.trim()]) };
}"""
def key(p, w): p.locator(f'#app [aria-label="Bouton {w}"]').click(); p.wait_for_timeout(100)
def sim(p, t): p.locator('#app button', has_text=t).first.click(); p.wait_for_timeout(100)
with sync_playwright() as pw:
    b = pw.chromium.launch(executable_path=os.environ.get('CHROMIUM') or None)
    p = b.new_page(viewport={'width': 900, 'height': 800})
    errs = []; p.on('pageerror', lambda e: errs.append(str(e)))
    p.goto('file://' + os.path.abspath(sys.argv[1])); p.wait_for_timeout(1500)
    st = {}
    st['home_search'] = p.evaluate(JS)
    p.wait_for_timeout(6200); st['home_ok'] = p.evaluate(JS)
    sim(p, 'Se mettre'); st['home_rolling'] = p.evaluate(JS)
    key(p, 'bas droite')
    for i in range(3):
        st[f'ride_p{i}'] = p.evaluate(JS); key(p, 'haut gauche'); st[f'ride_p{i}_hg'] = p.evaluate(JS); key(p, 'bas gauche')
    sim(p, 'Tunnel'); st['gps_lost'] = p.evaluate(JS); sim(p, 'Sortir du tunnel')
    sim(p, 'Sortir de la trace'); st['off_route'] = p.evaluate(JS); sim(p, 'Revenir')
    sim(p, 'Montée : approche'); st['climb_approach'] = p.evaluate(JS); sim(p, 'Montée : pied'); st['climb_in'] = p.evaluate(JS)
    key(p, 'bas droite'); st['pause'] = p.evaluate(JS)
    key(p, 'haut gauche'); st['pause_menu'] = p.evaluate(JS); key(p, 'haut gauche')
    key(p, 'bas gauche'); st['confirm'] = p.evaluate(JS)
    key(p, 'bas droite'); st['saved'] = p.evaluate(JS)
    key(p, 'bas droite'); key(p, 'bas gauche'); st['menu'] = p.evaluate(JS)
    key(p, 'bas droite'); st['menu_open1'] = p.evaluate(JS)
    b.close()
geo = {}
for k, v in st.items():
    g = json.dumps((v['top'], v['bar'], v['right']))
    geo.setdefault(g, []).append(k)
    flag = ' <-- CUT' if (v['cut'] or v['rightCut']) else ''
    print(f"{k:16} {v['labels']} cut={v['cut']}{' rightCut' if v['rightCut'] else ''}{flag}")
print('distinct geometries:', len(geo))
for g, ks in geo.items(): print(' ', g, ks)
print('errors:', errs)

sys.exit(0 if len(geo) == 1 and not errs else 1)
