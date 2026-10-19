#!/usr/bin/env -S uv run -q --with littlefs-python --with garmin-fit-sdk python3
"""Copy the rides (and the raw sensor logs /logs/*.csv) recorded by opencc600 to the PC and check them with the official Garmin FIT SDK.

Reads the storage partition (read only: `esptool read-flash`, the device then resets, which also tests the
recovery of a ride being recorded), mounts our littlefs on the PC and copies /rides/* to OUT_DIR
(`<unix start>.fit` closed rides, `.part` interrupted ones, closed at the next boot).

usage: pull_rides.py [OUT_DIR]          (default dump/rides; contains real positions: keep it out of git)
       pull_rides.py --image IMAGE.bin [OUT_DIR]
"""
import argparse
import subprocess
import sys
from pathlib import Path

STORAGE = (0x860000, 0x79E000)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out", nargs="?", type=Path, default=Path("dump/rides"))
    ap.add_argument("--image", type=Path, help="partition image already read")
    ap.add_argument("--port", default="/dev/ttyACM0")
    a = ap.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    image = a.image or a.out / "storage.bin"
    if not a.image:
        cmd = ["esptool", "--chip", "esp32s3", "-p", a.port, "read-flash", hex(STORAGE[0]), hex(STORAGE[1]), str(image)]
        print("+", " ".join(cmd))
        subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)

    from garmin_fit_sdk import Decoder, Stream
    from littlefs import LittleFS

    data = image.read_bytes()
    fs = LittleFS(block_size=4096, block_count=len(data) // 4096, mount=False, name_max=255)
    fs.context.buffer = bytearray(data)
    fs.mount()
    names = sorted(fs.listdir("/rides")) if "rides" in fs.listdir("/") else []
    ok = True
    for name in names:
        with fs.open(f"/rides/{name}", "rb") as f:
            body = f.read()
        (a.out / name).write_bytes(body)
        if not name.endswith(".fit"):
            print(f"{name}: {len(body)} bytes, interrupted (closed at the next boot)")
            continue
        integrity = Decoder(Stream.from_byte_array(bytearray(body))).check_integrity()
        messages, errors = Decoder(Stream.from_byte_array(bytearray(body))).read()
        counts = {k.removesuffix("_mesgs"): len(v) for k, v in messages.items()}
        session = (messages.get("session_mesgs") or [{}])[0]
        print(
            f"{name}: {len(body)} bytes, integrity {'ok' if integrity else 'FAILED'}, {counts}, "
            f"timer {session.get('total_timer_time')} s, {session.get('total_distance')} m, "
            f"+{session.get('total_ascent')} m"
        )
        for e in errors:
            print(f"  error: {e}")
        ok &= integrity and not errors
    if not names:
        print("no rides")
    # raw sensor logs of the firmware (/logs/<start>.csv, real positions: stay in dump/)
    for name in sorted(fs.listdir("/logs")) if "logs" in fs.listdir("/") else []:
        with fs.open(f"/logs/{name}", "rb") as f:
            body = f.read()
        (a.out / name).write_bytes(body)
        lines = body.count(b"\n") - 1
        print(f"log {name}: {lines} lines")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
