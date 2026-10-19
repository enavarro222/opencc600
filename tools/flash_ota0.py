#!/usr/bin/env python3
"""Write an app image to the CC600 ota_0 slot, and nothing else; or put the vendor firmware back.

Safety rules (docs/REVERSE_NOTES.md): never eFuses, never the bootloader (0x0)
or the partition table (0x8000); only ota_0 (+ otadata), and the storage
partition only to put the vendor's back. The vendor firmware stays in ota_1 as
a fallback.

- reads the partition table from the device and refuses to go on unless it is
  the known CC600 layout (ota_0 at 0x20000, 0x420000 bytes; otadata at 0xd000);
- saves the original otadata once to dump/otadata.orig.bin (never overwritten);
- writes the image to ota_0, erases otadata so the bootloader picks ota_0.

usage: flash_ota0.py IMAGE.bin [-p /dev/ttyACM0] [--dry-run]
       flash_ota0.py --restore-vendor [-p /dev/ttyACM0]   (boot ota_1 again)
       flash_ota0.py --restore-vendor --storage dump/storage.orig.bin
                     (also write the vendor storage partition back: rides, settings, pairings)
"""
import argparse
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

OTA0 = (0x20000, 0x420000)
OTADATA = (0xD000, 0x2000)
STORAGE = (0x860000, 0x79E000)
PART_TABLE = (0x8000, 0xC00)
ALLOWED_WRITES = {OTA0[0], OTADATA[0], STORAGE[0]}

DUMP = Path(__file__).resolve().parent.parent / "dump"
OTADATA_ORIG = DUMP / "otadata.orig.bin"


def esptool(port, *args, after="hard-reset"):
    cmd = ["esptool", "--chip", "esp32s3", "-p", port, "--after", after, *args]
    print("+", " ".join(cmd))
    subprocess.run(cmd, check=True)


def write(port, offset, path, after="hard-reset"):
    if offset not in ALLOWED_WRITES:
        sys.exit(f"refusing to write at {offset:#x}")
    esptool(port, "write-flash", f"{offset:#x}", str(path), after=after)


def read_partitions(port):
    with tempfile.TemporaryDirectory() as d:
        f = Path(d) / "pt.bin"
        esptool(port, "read-flash", f"{PART_TABLE[0]:#x}", f"{PART_TABLE[1]:#x}", str(f), after="no-reset")
        raw = f.read_bytes()
    parts = {}
    for i in range(0, len(raw), 32):
        e = raw[i:i + 32]
        if e[:2] != b"\xaa\x50":
            break
        _, ptype, sub, off, size, name, _ = struct.unpack("<2sBBII16sI", e)
        parts[name.rstrip(b"\0").decode()] = (ptype, sub, off, size)
    return parts


def check_layout(parts):
    ota0, otadata, storage = parts.get("ota_0"), parts.get("otadata"), parts.get("storage")
    if not storage or (storage[0], storage[1], storage[2], storage[3]) != (1, 0x83, *STORAGE):
        sys.exit(f"unexpected storage partition {storage}, not the known CC600 layout")
    if not ota0 or (ota0[0], ota0[2], ota0[3]) != (0, *OTA0):
        sys.exit(f"unexpected ota_0 partition {ota0}, not the known CC600 layout")
    if not otadata or (otadata[0], otadata[1], otadata[2], otadata[3]) != (1, 0, *OTADATA):
        sys.exit(f"unexpected otadata partition {otadata}, not the known CC600 layout")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("image", nargs="?", type=Path)
    ap.add_argument("-p", "--port", default="/dev/ttyACM0")
    ap.add_argument("--dry-run", action="store_true", help="check only, write nothing")
    ap.add_argument("--restore-vendor", action="store_true", help="write the original otadata back (boots ota_1)")
    ap.add_argument("--storage", type=Path, help="with --restore-vendor: storage partition image to write back")
    a = ap.parse_args()

    if a.restore_vendor:
        if not OTADATA_ORIG.exists():
            sys.exit(f"{OTADATA_ORIG} not found")
        if a.storage:
            img = a.storage.read_bytes()
            if len(img) != STORAGE[1] or img[8:16] != b"littlefs":
                sys.exit(f"{a.storage} is not a {STORAGE[1]:#x}-byte littlefs image")
            check_layout(read_partitions(a.port))
            write(a.port, STORAGE[0], a.storage, after="no-reset")
        write(a.port, OTADATA[0], OTADATA_ORIG)
        return
    if a.storage:
        ap.error("--storage only goes with --restore-vendor")
    if not a.image:
        ap.error("IMAGE is required")

    size = a.image.stat().st_size
    if a.image.read_bytes()[:1] != b"\xe9":
        sys.exit(f"{a.image} is not an ESP app image (magic 0xE9)")
    if size > OTA0[1]:
        sys.exit(f"image is {size:#x} bytes, ota_0 holds {OTA0[1]:#x}")

    check_layout(read_partitions(a.port))
    print(f"layout OK, image {size:#x} / {OTA0[1]:#x} bytes")

    if not OTADATA_ORIG.exists():
        DUMP.mkdir(exist_ok=True)
        esptool(a.port, "read-flash", f"{OTADATA[0]:#x}", f"{OTADATA[1]:#x}", str(OTADATA_ORIG), after="no-reset")
        if set(OTADATA_ORIG.read_bytes()) == {0xFF}:
            OTADATA_ORIG.unlink()
            sys.exit("otadata is already erased (not the vendor one): restore the vendor otadata first")
        print(f"original otadata saved to {OTADATA_ORIG}")

    if a.dry_run:
        esptool(a.port, "read-mac")  # hard reset: leave the ROM loader, boot again
        print("dry run: nothing written")
        return
    write(a.port, OTA0[0], a.image, after="no-reset")
    esptool(a.port, "erase-region", f"{OTADATA[0]:#x}", f"{OTADATA[1]:#x}")


if __name__ == "__main__":
    main()
