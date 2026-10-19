#!/usr/bin/env python3
"""Rides a GPX on the CC600 without going out: one position a second over USB (the firmware's `fix` command), in
place of the receiver's, along the track. With the track's times it keeps their pace, else `--kmh`; `--speedup`
goes faster (the positions jump further each second). The receiver takes over again at the end (`fix off`), or
5 s after the last position if the script is stopped.

usage: uv run --project tests/hil tools/replay_gpx.py FILE.gpx [-p /dev/ttyACM0] [--speedup 5] [--kmh 20]
       [--from-km 12]
"""
import argparse
import math
import signal
import sys
import time
import xml.etree.ElementTree as ET
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tests/hil"))
from dut import Dut  # noqa: E402

EARTH_M = 6_371_000.0


def read_gpx(path: Path) -> list[tuple[float, float, float | None]]:
    """(lat, lon, unix time or None) of the track points (or the route points)."""
    points = []
    for el in ET.parse(path).iter():
        if el.tag.rsplit("}", 1)[-1] not in ("trkpt", "rtept"):
            continue
        t = next((c.text for c in el if c.tag.rsplit("}", 1)[-1] == "time"), None)
        unix = datetime.fromisoformat(t.strip().replace("Z", "+00:00")).timestamp() if t else None
        points.append((float(el.get("lat")), float(el.get("lon")), unix))
    return points


def distance_m(a, b) -> float:
    (la1, lo1), (la2, lo2) = (map(math.radians, a[:2]), map(math.radians, b[:2]))
    h = math.sin((la2 - la1) / 2) ** 2 + math.cos(la1) * math.cos(la2) * math.sin((lo2 - lo1) / 2) ** 2
    return 2 * EARTH_M * math.asin(min(1.0, math.sqrt(h)))


def bearing_cdeg(a, b) -> int:
    la1, lo1, la2, lo2 = map(math.radians, (a[0], a[1], b[0], b[1]))
    y = math.sin(lo2 - lo1) * math.cos(la2)
    x = math.cos(la1) * math.sin(la2) - math.sin(la1) * math.cos(la2) * math.cos(lo2 - lo1)
    return round(math.degrees(math.atan2(y, x)) % 360 * 100) % 36000


def fixes(points, kmh: float = 20.0, speedup: float = 1.0, from_km: float = 0.0):
    """One (lat_e7, lon_e7, speed_mm_s, course_cdeg) a second along the points: at the track's pace when every
    point has a time, else at `kmh`; `speedup` times faster. Speed and course are those between the positions sent."""
    along = [0.0]
    for a, b in zip(points, points[1:]):
        along.append(along[-1] + distance_m(a, b))
    timed = all(p[2] is not None for p in points)
    # time of each point, seconds from the start
    t = [p[2] - points[0][2] for p in points] if timed else [d / (kmh / 3.6) for d in along]
    i = 0
    while i + 1 < len(along) and along[i + 1] < from_km * 1000:
        i += 1
    now, last, course = t[i], None, None
    while True:
        while i + 1 < len(t) and t[i + 1] <= now:
            i += 1
        if i + 1 >= len(t):
            p = points[-1][:2]
        else:
            f = (now - t[i]) / max(t[i + 1] - t[i], 1e-9)
            a, b = points[i], points[i + 1]
            p = (a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f)
        moved = distance_m(last, p) if last else 0.0
        if moved > 0.5:
            course = bearing_cdeg(last, p)
        yield round(p[0] * 1e7), round(p[1] * 1e7), round(moved * 1000), course
        if i + 1 >= len(t):
            return
        last, now = p, now + speedup


def command(fix) -> str:
    lat, lon, speed, course = fix
    return f"fix {lat} {lon} {speed}" + (f" {course}" if course is not None else "")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("gpx", type=Path)
    ap.add_argument("-p", "--port", default="/dev/ttyACM0")
    ap.add_argument("--kmh", type=float, default=20.0, help="pace without times in the GPX (default 20)")
    ap.add_argument("--speedup", type=float, default=1.0, help="track seconds a second (default 1)")
    ap.add_argument("--from-km", type=float, default=0.0, help="start this far along the track")
    a = ap.parse_args()
    points = read_gpx(a.gpx)
    if len(points) < 2:
        ap.error("no track in the GPX")
    dut = Dut(a.port)
    # stopped by a signal (a background job) as by Ctrl-C: the receiver back at once
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt))
    start = time.monotonic()
    try:
        for n, fix in enumerate(fixes(points, a.kmh, a.speedup, a.from_km)):
            dut.send(command(fix))
            print(f"\r{n} s, {fix[2] * 36 / 10_000:5.1f} km/h", end="", flush=True)
            time.sleep(max(0.0, start + n + 1 - time.monotonic()))
    except KeyboardInterrupt:
        pass
    finally:
        dut.send("fix off")
        print("\nreceiver back")


if __name__ == "__main__":
    main()
