#!/usr/bin/env python3
"""Sends a route to the CC600 over its USB serial port (the firmware's test hook: `route begin`, `route data`,
`route end`; numbered and acknowledged lines, see tests/hil/dut.py). A GPX is turned into a route first with
`route-tool build` (cargo). `--clear` removes the route.

usage: uv run --project tests/hil tools/send_route.py FILE.gpx|FILE.route [-p /dev/ttyACM0]
       uv run --project tests/hil tools/send_route.py --clear [-p /dev/ttyACM0]
"""
import argparse
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tests/hil"))
from dut import Dut  # noqa: E402

CHUNK = 100  # bytes a line, 200 hex digits (the firmware's line holds 256 characters)


def crc16(data: bytes) -> int:
    """CRC-16/CCITT-FALSE, like crates/route: the firmware acknowledges a data line only if it matches."""
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def route_bytes(path: Path) -> bytes:
    if path.suffix.lower() != ".gpx":
        return path.read_bytes()
    with tempfile.TemporaryDirectory() as d:
        out = Path(d) / "route.bin"
        subprocess.run(["cargo", "run", "-q", "--release", "-p", "route-tool", "--", "build", str(path), str(out)],
                       check=True, cwd=ROOT)
        return out.read_bytes()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("file", nargs="?", type=Path)
    ap.add_argument("-p", "--port", default="/dev/ttyACM0")
    ap.add_argument("--clear", action="store_true")
    a = ap.parse_args()
    dut = Dut(a.port)
    if a.clear:
        dut.send("route clear")
        dut.expect(r"route: removed", timeout=5)
        print("route removed")
        return
    if not a.file:
        ap.error("a file, or --clear")
    t0 = time.time()
    result = send(dut, route_bytes(a.file),
                  lambda done, n: print(f"\r{done} / {n} bytes", end="", flush=True))
    print(f"\n{result} ({time.time() - t0:.0f} s)")


def send(dut: Dut, data: bytes, progress=lambda done, n: None) -> str:
    """Sends a route file; the firmware's answer (`loaded ...`, or why it refused it)."""
    dut.send(f"route begin {len(data)}")
    for i in range(0, len(data), CHUNK):
        chunk = data[i:i + CHUNK]
        dut.send(f"route data {chunk.hex()}{crc16(chunk):04x}")
        progress(min(i + CHUNK, len(data)), len(data))
    mark = dut.mark()
    dut.send("route end")
    m = dut.expect(r"route: (loaded .*|received file refused.*|write failed.*)", timeout=10, since=mark)
    return m.group(1)


if __name__ == "__main__":
    main()
