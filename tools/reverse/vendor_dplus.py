#!/usr/bin/env -S uv run -q --with garmin-fit-sdk python3
"""Replays the vendor ascent / descent state machine (decompiled from ota_1, docs/REVERSE_NOTES.md) on the
altitude of vendor FIT files, and compares with their session totals and with a plain 2 m hysteresis.

usage: vendor_dplus.py FILE.fit...     (vendor rides, e.g. dump/fits/*.fit from tools/reverse/vendor_fit.py)
"""
import sys
from garmin_fit_sdk import Decoder, Stream

START, MIN_SEG = 5, 11  # dm

def ascent(alts_dm, sign=1):
    total = 0.0; state = 0; bottom = top = None
    for a in alts_dm:
        a = sign * a
        if bottom is None: bottom = top = a
        if state == 0:
            if a - bottom > START: state = 1; bottom = top = a
            elif a < bottom: bottom = a
        else:
            if a > top: top = a
            if top - a >= START:
                if top - bottom >= MIN_SEG: total += top - bottom
                state = 0; bottom = top = a
    if state == 1 and top - bottom >= MIN_SEG: total += top - bottom
    return total / 10

def hyst(alts_dm, th):
    up = 0; ref = alts_dm[0]
    for a in alts_dm:
        if a >= ref + th: up += a - ref; ref = a
        elif a <= ref - th: ref = a
    return up / 10

rows = []
for path in sys.argv[1:]:
    m, _ = Decoder(Stream.from_file(path)).read(convert_datetimes_to_dates=False)
    alts = [round(r['enhanced_altitude'] * 10) for r in m.get('record_mesgs', []) if r.get('enhanced_altitude') is not None]
    if len(alts) < 100: continue
    s = m['session_mesgs'][0]
    va, vd = ascent(alts), ascent(alts, -1)
    rows.append((s['total_ascent'], va, s['total_descent'], vd, hyst(alts, 20)))
    print(f"{path.split('/')[-1]}: vendor +{s['total_ascent']:5} -{s['total_descent']:5} | replayed +{va:7.1f} -{vd:7.1f} | ours (2 m) +{hyst(alts, 20):7.1f}")
ea = [abs(r[1] - r[0]) / max(r[0], 1) for r in rows]; ed = [abs(r[3] - r[2]) / max(r[2], 1) for r in rows]
print('median rel. error ascent %.1f %%, descent %.1f %%' % (100 * sorted(ea)[len(ea)//2], 100 * sorted(ed)[len(ed)//2]))
print('total vendor +%d, replayed +%d, ours 2 m +%d' % (sum(r[0] for r in rows), sum(r[1] for r in rows), sum(r[4] for r in rows)))
