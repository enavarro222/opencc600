#!/usr/bin/env -S uv run -q --with pyserial python3
"""Measure the GNSS time to first fix (TTFF) after forced restarts, through the firmware's USB test hook.

Run it with the device by a window (or outdoors), plugged in. Each run sets the time aiding on or off
(`gnss time on|off`: the firmware answers the receiver with the RTC time, `$PAIR590`), forces a restart
(`gnss hot|warm|cold|full`, `$PAIR004`..`$PAIR007`) and waits for the firmware's `gnss: first fix N s` line.
After each fix it waits --settle seconds so the receiver can download the ephemerides (needed by a hot start).

Cases: `full` (full cold start), `full+time` (full cold, then the RTC time), `cold`, `cold+time`, `warm`, `hot`.
Results are appended to dump/ttff.csv (no position in it) and summarised at the end.

usage: ttff.py [-p /dev/ttyACM0] [--cases full,full+time,hot] [--runs 3] [--timeout 900] [--settle 60]
"""
import argparse
import csv
import re
import statistics
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tests" / "hil"))
from dut import Dut  # noqa: E402

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("-p", "--port", default="/dev/ttyACM0")
ap.add_argument("--cases", default="full,full+time,hot")
ap.add_argument("--runs", type=int, default=3, help="runs per case (cases interleaved)")
ap.add_argument("--timeout", type=float, default=900, help="give up on a run after this many seconds")
ap.add_argument("--settle", type=float, default=60, help="seconds with a fix before the next restart")
a = ap.parse_args()

cases = a.cases.split(",")
for case in cases:
    start, _, aid = case.partition("+")
    if start not in ("hot", "warm", "cold", "full") or aid not in ("", "time"):
        sys.exit(f"unknown case {case!r}")

out = ROOT / "dump" / "ttff.csv"
new = not out.exists()
dut = Dut(a.port, log_path=ROOT / "dump" / f"ttff-{time.strftime('%Y%m%d-%H%M%S')}.log")
results: dict[str, list[float | None]] = {c: [] for c in cases}
try:
    print("waiting for a first fix before starting...", flush=True)
    dut.expect(r"gnss: .*fix true", timeout=a.timeout)
    time.sleep(a.settle)
    with open(out, "a", newline="") as f:
        w = csv.writer(f)
        if new:
            w.writerow(["host_time", "case", "ttff_s", "sats_used", "sats_in_view", "time_aid_ack"])
        for run in range(a.runs):
            for case in cases:
                start, _, aid = case.partition("+")
                dut.send(f"gnss time {'on' if aid else 'off'}")
                mark = dut.mark()
                t0 = time.time()
                dut.send(f"gnss {start}")
                try:
                    m = dut.expect(r"gnss: first fix ([\d.]+) s after .*, (\d+)/(\d+) sats", timeout=a.timeout,
                                   since=mark)
                    ttff, used, view = float(m[1]), int(m[2]), int(m[3])
                except TimeoutError:
                    ttff, used, view = None, "", ""
                ack = ""
                if aid:
                    try:
                        ack = dut.expect(r"gnss: PAIR590 -> (\d+)", timeout=0.1, since=mark)[1]
                    except TimeoutError:
                        ack = "none"
                shown = "timeout" if ttff is None else f"{ttff:.1f} s"
                print(f"run {run + 1}/{a.runs} {case:10} {shown:>8}  {used}/{view} sats"
                      f"{f'  PAIR590 ack {ack}' if aid else ''}  ({time.time() - t0:.0f} s)", flush=True)
                w.writerow([time.strftime("%Y-%m-%dT%H:%M:%S"), case, "" if ttff is None else ttff, used, view, ack])
                f.flush()
                results[case].append(ttff)
                time.sleep(a.settle)
finally:
    dut.send("gnss time off")
    dut.close()

print("\ncase        median     runs")
for case, values in results.items():
    ok = [v for v in values if v is not None]
    med = f"{statistics.median(ok):.1f} s" if ok else "-"
    print(f"{case:10} {med:>8}   {', '.join('timeout' if v is None else f'{v:.1f}' for v in values)}")
