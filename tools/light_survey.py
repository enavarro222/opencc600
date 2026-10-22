#!/usr/bin/env -S uv run -q --with pyserial python3
"""Measure the ambient light sensor in a few scenes, on one or more devices at once, through the USB test hook.

For each scene (set by hand: dark, a room's light, a headlamp on the devices...) the script waits for Enter, then on
every device sets the backlight by hand to each of --backlights in turn (`auto_backlight off`) and polls `light ?`
for --seconds: the sensor's raw levels (tags 600, 2000 and 3000) per scene and backlight. A level that follows the
backlight means the sensor sees the screen. A last pass with `auto_backlight on` logs the backlight it picks.
The settings are put back at the end.

Results are appended to dump/light_survey.csv and summarised at the end (median, min and max per tag).

usage: light_survey.py [-p /dev/ttyACM0 -p /dev/ttyACM1] [--scenes noir,cuisine,frontale] [--backlights 10,100]
                       [--seconds 15]
"""
import argparse
import csv
import re
import statistics
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tests" / "hil"))
from dut import Dut  # noqa: E402

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("-p", "--port", action="append", help="a device's port (repeat for each device)")
ap.add_argument("--scenes", default="noir,cuisine,frontale")
ap.add_argument("--backlights", default="10,100", help="backlights set by hand, percent")
ap.add_argument("--seconds", type=float, default=15, help="polling time per scene and backlight")
a = ap.parse_args()

ports = a.port or ["/dev/ttyACM0"]
scenes = a.scenes.split(",")
backlights = [int(b) for b in a.backlights.split(",")]
KEYS = ("backlight", "auto_backlight")
LINE = re.compile(r"light: theme \w+, backlight (\d+) %, sensor \[(.*)\]")


def get(dut: Dut, key: str) -> str:
    for _ in range(5):
        mark = dut.mark()
        dut.send(f"get {key}")
        try:
            return dut.expect(rf"usb: {key}=(\S+)", timeout=1.0, since=mark).group(1)
        except TimeoutError:
            continue
    raise TimeoutError(f"no value for {key}")


def poll(dut: Dut, seconds: float) -> list[tuple[int, list]]:
    """(backlight, [raw 600, raw 2000, raw 3000]) every ~0.3 s, the sensor's period."""
    out = []
    end = time.time() + seconds
    while time.time() < end:
        mark = dut.mark()
        dut.send("light ?")
        try:
            m = dut.expect(LINE.pattern, timeout=1.0, since=mark)
        except TimeoutError:
            continue  # the log line was lost
        raw = [int(re.search(r"\d+", s).group()) if "Some" in s else None for s in m.group(2).split(",")]
        out.append((int(m.group(1)), raw))
        time.sleep(0.3)
    return out


def measure(dut: Dut, port: str, scene: str, mode: str, rows: list):
    if mode != "auto":
        dut.set("backlight", mode)
    time.sleep(3)  # the backlight set, the sensor's average of the levels before gone
    for pct, raw in poll(dut, a.seconds):
        rows.append({"device": port, "scene": scene, "mode": mode, "backlight": pct,
                     "raw600": raw[0], "raw2000": raw[1], "raw3000": raw[2]})


duts = {p: Dut(p) for p in ports}
before = {p: {k: get(d, k) for k in KEYS} for p, d in duts.items()}
rows: list[dict] = []
try:
    for scene in scenes:
        input(f"\nscene « {scene} » : mets-la en place puis Entrée ")
        for mode in [*map(str, backlights), "auto"]:
            for d in duts.values():
                d.send(f"set auto_backlight {'on' if mode == 'auto' else 'off'}")
            print(f"  {scene}, backlight {mode}...", flush=True)
            threads = [threading.Thread(target=measure, args=(d, p, scene, mode, rows)) for p, d in duts.items()]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
finally:
    for p, d in duts.items():
        for k, v in before[p].items():
            d.set(k, v)
        d.close()

out = ROOT / "dump" / "light_survey.csv"
out.parent.mkdir(exist_ok=True)
new = not out.exists()
with out.open("a", newline="") as f:
    w = csv.DictWriter(f, fieldnames=["at", *rows[0]] if rows else ["at"])
    if new:
        w.writeheader()
    at = time.strftime("%Y-%m-%d %H:%M:%S")
    w.writerows({"at": at, **r} for r in rows)
print(f"\n{len(rows)} mesures dans {out}\n")


def stats(vs: list) -> str:
    vs = [v for v in vs if v is not None]
    return f"{statistics.median(vs):6.0f} [{min(vs)}-{max(vs)}]" if vs else "     -"


print(f"{'device':14} {'scene':10} {'mode':5} {'backlight':>9}  {'raw600':>16} {'raw2000':>16} {'raw3000':>16}")
for p in ports:
    for scene in scenes:
        for mode in [*map(str, backlights), "auto"]:
            rs = [r for r in rows if (r["device"], r["scene"], r["mode"]) == (p, scene, mode)]
            if not rs:
                continue
            bl = stats([r["backlight"] for r in rs])
            print(f"{p[-12:]:14} {scene:10} {mode:5} {bl:>9}  "
                  + " ".join(f"{stats([r[k] for r in rs]):>16}" for k in ("raw600", "raw2000", "raw3000")))
