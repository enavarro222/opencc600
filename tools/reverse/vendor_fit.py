#!/usr/bin/env -S uv run -q --with littlefs-python python3
"""Extract the rides recorded by the vendor firmware as standard FIT files.

The vendor stores rides in its littlefs `storage` partition as /FITS/FIT/<unix start time>.fit, in its own
compressed container:

    ".ZIP"  FIT file header (14 bytes, ".FIT" at 8)  { u16 LE length, zlib stream }*  FIT CRC (2 bytes)

The zlib chunks hold the FIT records (the header's data size, once inflated). Header + records + CRC is the
original FIT file, byte for byte.

usage: vendor_fit.py STORAGE.bin OUT_DIR     (raw image of the storage partition, e.g. dump/<mac>/storage.orig.bin)
       vendor_fit.py --container FILE.fit... OUT_DIR
"""
import argparse
import struct
import sys
import zlib
from pathlib import Path


def unpack(container: bytes) -> bytes:
    if container[:4] != b".ZIP" or container[12:16] != b".FIT":
        raise ValueError("not a vendor FIT container")
    header = container[4:18]
    size = struct.unpack("<I", header[4:8])[0]
    pos, body = 18, b""
    while len(body) < size:
        (n,) = struct.unpack("<H", container[pos : pos + 2])
        body += zlib.decompress(container[pos + 2 : pos + 2 + n])
        pos += 2 + n
    if len(body) != size or pos != len(container) - 2:
        raise ValueError(f"inconsistent container: {len(body)} / {size} bytes, end at {pos} / {len(container)}")
    return header + body + container[-2:]


def from_image(image: Path):
    from littlefs import LittleFS

    data = image.read_bytes()
    fs = LittleFS(block_size=4096, block_count=len(data) // 4096, mount=False, name_max=255)
    fs.context.buffer = bytearray(data)
    fs.mount()
    for name in sorted(fs.listdir("/FITS/FIT")):
        with fs.open(f"/FITS/FIT/{name}", "rb") as f:
            yield name, f.read()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("source", nargs="+", type=Path)
    ap.add_argument("--container", action="store_true", help="sources are container files, not a partition image")
    a = ap.parse_args()
    *sources, out = a.source
    if not sources:
        ap.error("missing source")
    out.mkdir(parents=True, exist_ok=True)
    files = ((p.name, p.read_bytes()) for p in sources) if a.container else from_image(sources[0])
    n = 0
    for name, container in files:
        try:
            (out / name).write_bytes(unpack(container))
            n += 1
        except ValueError as e:
            print(f"{name}: {e}", file=sys.stderr)
    print(f"{n} FIT files written to {out}")


if __name__ == "__main__":
    main()
