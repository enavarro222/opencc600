#!/usr/bin/env python3
"""The builds of the in-browser demo (docs/demo/builds/<name>/) and their index, versions.json.

usage: versions.py build-info BUILD_DIR NAME COMMIT ESP32SIM_COMMIT   (writes BUILD_DIR/build.json)
       versions.py index DEMO_DIR                                     (writes DEMO_DIR/versions.json)
       versions.py fetch URL DEMO_DIR [--skip NAME]   (downloads the builds a published demo lists, so that
                                                       a new deployment keeps them)

versions.json lists `main` first, then the other builds newest first.
"""
import json
import subprocess
import sys
import urllib.request
from datetime import datetime, timezone
from pathlib import Path


def build_info(build_dir, name, commit, sim):
    build = Path(build_dir)
    files = sorted(str(p.relative_to(build)) for p in build.rglob('*') if p.is_file() and p.name != 'build.json')
    date = subprocess.check_output(['git', 'show', '-s', '--format=%cI', commit], text=True).strip()
    info = {'name': name, 'commit': commit, 'date': date, 'esp32sim': sim, 'files': files,
            'built': datetime.now(timezone.utc).isoformat(timespec='seconds')}
    (build / 'build.json').write_text(json.dumps(info, indent=1) + '\n')


def index(demo_dir):
    demo = Path(demo_dir)
    builds = [json.loads(p.read_text()) for p in sorted((demo / 'builds').glob('*/build.json'))]
    builds.sort(key=lambda b: b['date'], reverse=True)
    builds.sort(key=lambda b: b['name'] != 'main')
    keep = ('name', 'commit', 'date', 'esp32sim')
    (demo / 'versions.json').write_text(json.dumps([{k: b[k] for k in keep} for b in builds], indent=1) + '\n')
    print('versions:', ', '.join(b['name'] for b in builds))


def get(url):
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read()


def fetch(url, demo_dir, skip):
    demo = Path(demo_dir)
    try:
        versions = json.loads(get(f'{url}/versions.json'))
    except Exception as e:  # nothing published yet
        print(f'no published versions ({e})')
        return
    for v in versions:
        name = v['name']
        if name == skip or '/' in name or name.startswith('.'):
            continue
        base = f'{url}/builds/{name}'
        info = json.loads(get(f'{base}/build.json'))
        out = demo / 'builds' / name
        for f in info['files']:
            path = out / f
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(get(f'{base}/{f}'))
        (out / 'build.json').write_text(json.dumps(info, indent=1) + '\n')
        print('kept', name)


if __name__ == '__main__':
    a = sys.argv[1:]
    if a[:1] == ['build-info'] and len(a) == 5:
        build_info(*a[1:])
    elif a[:1] == ['index'] and len(a) == 2:
        index(a[1])
    elif a[:1] == ['fetch'] and len(a) in (3, 5) and (len(a) == 3 or a[3] == '--skip'):
        fetch(a[1].rstrip('/'), a[2], a[4] if len(a) == 5 else None)
    else:
        sys.exit(__doc__)
