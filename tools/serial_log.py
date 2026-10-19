#!/usr/bin/env -S uv run -q --with pyserial python3
"""Print the CC600 USB serial log for a while, each line prefixed with the host time in seconds.

The ESP32-S3 USB-Serial/JTAG port ignores the baud rate. Does not reset the device, unless --reset (then the log
starts at the bootloader).

usage: serial_log.py [SECONDS] [-p /dev/ttyACM0] [--reset]
"""
import argparse
import re
import sys
import time

import serial

ANSI = re.compile(rb"\x1b\[[0-9;]*m")

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("seconds", nargs="?", type=float, default=10)
ap.add_argument("-p", "--port", default="/dev/ttyACM0")
ap.add_argument("--reset", action="store_true", help="reset the ESP32 first (RTS pulse, like esptool's hard reset)")
a = ap.parse_args()

s = serial.Serial(a.port, 115200, timeout=0.2)
if a.reset:
    s.dtr = False
    s.rts = True
    time.sleep(0.1)
    s.rts = False
t0 = time.time()
buf = b""
while time.time() - t0 < a.seconds:
    buf += s.read(4096)
    *lines, buf = buf.split(b"\n")
    for line in lines:
        text = ANSI.sub(b"", line).decode("utf-8", "replace").rstrip()
        print(f"{time.time() - t0:8.3f} {text}", flush=True)
sys.exit(0)
