#!/usr/bin/env -S uv run -q --with pyserial python3
"""Reset the CC600 at random moments while it records, and check that no acknowledged ride data is lost.

Needs the firmware built with `--features hil-storage` in ota_0 (see firmware/src/hil_storage.rs). The firmware logs
`hil: acked <id> <len>` after every append that returned Ok; after a reset it checks every ride against the known
pattern and logs `hil: ride <id> <unfinished|finished> <len> ok|BAD at <offset>`. This script resets the ESP32
(RTS pulse, like esptool's hard reset) a random 1-8 s after each boot and checks, for the ride being recorded,
that the length found is the last acked one or one more append (appends are atomic), with valid content.

A reset is not a brown-out: the flash finishes the word being programmed. A battery cut needs the case open.

usage: hil_storage.py [CYCLES] [-p /dev/ttyACM0] [--log dump/hil_storage.log]
"""
import argparse
import random
import re
import sys
import time
from pathlib import Path

import serial

ANSI = re.compile(rb"\x1b\[[0-9;]*m")
ACKED = re.compile(r"hil: acked (\d+) (\d+)")
RIDE = re.compile(r"hil: ride (\d+) (unfinished|finished) (\d+) (ok|BAD at \d+)")
BOOT = re.compile(r"hil: boot, ")
CHUNK = 512


def reset(s):
    s.dtr = False
    s.rts = True
    time.sleep(0.1)
    s.rts = False


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cycles", nargs="?", type=int, default=20)
    ap.add_argument("-p", "--port", default="/dev/ttyACM0")
    ap.add_argument("--log", type=Path, default=Path(__file__).resolve().parent.parent / "dump" / "hil_storage.log")
    a = ap.parse_args()

    s = serial.Serial(a.port, 115200, timeout=0.1)
    log = a.log.open("w")
    reset(s)
    t0 = time.time()
    buf = b""
    acked = None  # (id, len) last acknowledged before the current reset
    rides = {}  # id -> (state, len, result) reported at this boot
    booted_at = None
    reset_at = None
    cycle, failures, overflows, appends = 0, [], 0, 0
    while cycle <= a.cycles:
        buf += s.read(4096)
        *lines, buf = buf.split(b"\n")
        for raw in lines:
            line = ANSI.sub(b"", raw).decode("utf-8", "replace").rstrip()
            log.write(f"{time.time() - t0:8.3f} {line}\n")
            if m := RIDE.search(line):
                rides[int(m[1])] = (m[2], int(m[3]), m[4])
                if m[4] != "ok":
                    failures.append(f"cycle {cycle}: {line}")
            elif BOOT.search(line):
                if acked is not None:
                    failures += check(cycle, acked, rides)
                cycle += 1
                rides = {}
                booted_at = time.time()
                reset_at = booted_at + random.uniform(1, 8)
                print(f"cycle {cycle}/{a.cycles}: {line.split('hil: ')[1]}", flush=True)
            elif m := ACKED.search(line):
                acked = (int(m[1]), int(m[2]))
                appends += 1
            elif "FifoOverflowed" in line:
                overflows += 1
            elif "panic" in line.lower() or "exception" in line.lower():
                failures.append(f"cycle {cycle}: {line}")
        if reset_at and time.time() >= reset_at and cycle <= a.cycles:
            reset_at = None
            if cycle < a.cycles:
                reset(s)
            else:
                break
        if booted_at is None and time.time() - t0 > 20:
            sys.exit("no 'hil: boot' line: is the hil-storage firmware in ota_0?")
    print(f"{a.cycles} resets, {appends} appends acked, {overflows} UART FIFO overflows (nRF), log in {a.log}")
    if failures:
        print("FAILED:\n  " + "\n  ".join(failures))
        sys.exit(1)
    print("OK: no acknowledged byte lost, every append atomic, all content valid")


def check(cycle, acked, rides):
    id_, len_ = acked
    found = rides.get(id_)
    if found is None:
        return [f"cycle {cycle}: ride {id_} (acked {len_} bytes) not found after reset; rides {rides}"]
    state, length, result = found
    if length not in (len_, len_ + CHUNK):
        return [f"cycle {cycle}: ride {id_} acked {len_} bytes, found {length} ({state})"]
    return []


if __name__ == "__main__":
    main()
