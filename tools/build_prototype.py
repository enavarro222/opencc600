#!/usr/bin/env python3
"""Build docs/prototype/index.html from docs/prototype/prototype.dc.html.

The .dc.html file is the clickable UI prototype exported from the design canvas.
This script wraps it in a standalone page with a tiny template runtime
(holes {{a.b}}, <sc-if>, <sc-for>, on* handlers), so it can be served by
GitHub Pages and shared with a plain link.

Usage: python3 tools/build_prototype.py
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "docs" / "prototype" / "prototype.dc.html"
OUT = ROOT / "docs" / "prototype" / "index.html"

src = SRC.read_text(encoding="utf-8")

xdc = re.search(r"<x-dc>(.*)</x-dc>", src, re.S).group(1)
helmet = re.search(r"<helmet>(.*?)</helmet>", xdc, re.S)
styles = "".join(re.findall(r"<style>(.*?)</style>", helmet.group(1), re.S)) if helmet else ""
template = re.sub(r"<helmet>.*?</helmet>", "", xdc, flags=re.S).strip()
logic = re.search(r"<script[^>]*data-dc-script[^>]*>(.*?)</script>", src, re.S).group(1)

RUNTIME = r"""
class DCLogic {
  constructor(props) { this.props = props || {}; this.state = {}; }
  setState(p) {
    const patch = typeof p === 'function' ? p(this.state) : p;
    this.state = Object.assign({}, this.state, patch);
    scheduleRender();
  }
  forceUpdate() { scheduleRender(); }
}

const tpl = document.getElementById('tpl').content;
const mount = document.getElementById('app');
let comp = null, pending = false;

function lookup(expr, scope) {
  expr = expr.trim();
  if (expr === 'true') return true;
  if (expr === 'false') return false;
  if (/^-?\d+(\.\d+)?$/.test(expr)) return Number(expr);
  return expr.split('.').reduce((o, k) => (o == null ? undefined : o[k]), scope);
}
const WHOLE = /^\{\{([^}]+)\}\}$/;
const interp = (str, scope) => str.replace(/\{\{([^}]+)\}\}/g, (_, e) => {
  const v = lookup(e, scope); return v == null ? '' : String(v);
});

function render(node, scope) {
  if (node.nodeType === 3) return [document.createTextNode(interp(node.nodeValue, scope))];
  if (node.nodeType !== 1) return [];
  const tag = node.localName;
  if (tag === 'sc-if') {
    const m = (node.getAttribute('value') || '').match(WHOLE);
    return m && lookup(m[1], scope) ? renderChildren(node, scope) : [];
  }
  if (tag === 'sc-for') {
    const m = (node.getAttribute('list') || '').match(WHOLE);
    const list = (m && lookup(m[1], scope)) || [];
    const as = node.getAttribute('as') || 'item';
    const out = [];
    list.forEach((it, i) => out.push(...renderChildren(node, Object.assign({}, scope, { [as]: it, $index: i }))));
    return out;
  }
  const el = node.namespaceURI && node.namespaceURI !== 'http://www.w3.org/1999/xhtml'
    ? document.createElementNS(node.namespaceURI, node.tagName)
    : document.createElement(tag);
  for (const a of node.attributes) {
    if (a.name.startsWith('hint-')) continue;
    const whole = a.value.match(WHOLE);
    if (a.name.startsWith('on') && whole) {
      const fn = lookup(whole[1], scope);
      if (typeof fn === 'function') el.addEventListener(a.name.slice(2), fn);
      continue;
    }
    el.setAttribute(a.name, interp(a.value, scope));
  }
  for (const c of renderChildren(node, scope)) el.appendChild(c);
  return [el];
}
function renderChildren(node, scope) {
  const out = [];
  const kids = node.content ? node.content.childNodes : node.childNodes;
  for (const c of kids) out.push(...render(c, scope));
  return out;
}
function draw() {
  pending = false;
  const vals = comp.renderVals();
  const nodes = [];
  for (const c of tpl.childNodes) nodes.push(...render(c, vals));
  mount.replaceChildren(...nodes);
}
function scheduleRender() { if (!pending) { pending = true; requestAnimationFrame(draw); } }

function start() {
  comp = new Component({ nuit: false });
  draw();
}

function wire() {
const night = document.getElementById('night');
night.addEventListener('change', () => { comp.props = { nuit: night.checked }; scheduleRender(); });

function fit() {
  const w = Math.min(1, (document.documentElement.clientWidth - 16) / 640);
  const box = document.getElementById('stage');
  box.style.transform = 'scale(' + w + ')';
  box.parentElement.style.height = (640 * w) + 'px';
}
window.addEventListener('resize', fit);
fit();
}
"""

PAGE = f"""<!doctype html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>opencc600 — prototype cliquable</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link href="https://fonts.googleapis.com/css2?family=Barlow+Condensed:wght@500;600;700&display=swap" rel="stylesheet">
<style>
:root {{ color-scheme: light; }}
body {{ margin: 0; background: #E9E7E2; color: #111; font-family: 'Barlow Condensed', sans-serif; }}
header {{ max-width: 640px; margin: 0 auto; padding: 20px 16px 8px; }}
h1 {{ margin: 0 0 4px; font-size: 30px; }}
header p {{ margin: 4px 0; font-size: 18px; line-height: 1.3; color: #333; }}
header a {{ color: #1D4ED8; }}
label.night {{ display: inline-flex; gap: 6px; align-items: center; font-size: 18px; font-weight: 600; margin-top: 6px; cursor: pointer; }}
.fitwrap {{ overflow: hidden; }}
#stage {{ width: 640px; height: 640px; margin: 0 auto; transform-origin: top left; }}
@media (min-width: 656px) {{ #stage {{ transform-origin: top center; }} }}
{styles}
</style>
</head>
<body>
<header>
  <h1>opencc600 — prototype cliquable</h1>
  <p>Maquette de l'interface du firmware libre pour le compteur GEOID CC600. Clique sur les 3 boutons bleus (ou sur les libellés à l'écran). Le panneau de droite simule les événements de la sortie.</p>
  <p>Écran réel : 240×320, 3 boutons. Code et docs : <a href="https://github.com/enavarro222/opencc600">github.com/enavarro222/opencc600</a></p>
  <label class="night"><input type="checkbox" id="night"> Mode nuit</label>
</header>
<div class="fitwrap"><div id="stage"><div id="app"></div></div></div>
<template id="tpl">
{template}
</template>
<script>
{RUNTIME}
{logic}
start();
wire();
</script>
</body>
</html>
"""

OUT.write_text(PAGE, encoding="utf-8")
print(f"wrote {OUT.relative_to(ROOT)} ({OUT.stat().st_size} bytes)")
