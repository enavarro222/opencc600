#!/usr/bin/env -S uv run -q --with littlefs-python python3
"""Extract and decrypt the nRF52810 application the vendor firmware pushes to the nRF (docs/NRF_FIRMWARE.md).

The vendor storage partition holds the update as /APP/<date>_<n>/N51_V<version>_A.bin: a 128-byte header XORed
with HEADER_KEY, then the application XORed with BODY_KEY (both repeat every 4 bytes). The output is the plain
application, to load at its address (0x12000, after the ANT SoftDevice): Ghidra raw binary, `ARM:LE:32:Cortex`.

usage: nrf_image.py STORAGE.bin OUT_DIR     (raw image of the storage partition, e.g. dump/<mac>/storage.orig.bin)
       nrf_image.py --file N51_V1.602_A.bin OUT_DIR
"""
import argparse
import struct
from pathlib import Path

HEADER_KEY = bytes([0x9A, 0x92, 0x45, 0x42])
BODY_KEY = bytes([0xDA, 0xDC, 0x05, 0x0C])
HEADER_LEN = 128
MAGIC = 0xAA555AA5


def xor(data: bytes, key: bytes) -> bytes:
    return bytes(b ^ key[i % len(key)] for i, b in enumerate(data))


def text(raw: bytes) -> str:
    return raw.split(b"\0", 1)[0].decode("ascii", "replace")


def decode(image: bytes) -> tuple[dict, bytes]:
    h = xor(image[:HEADER_LEN], HEADER_KEY)
    words = struct.unpack_from("<8I", h)
    if words[0] != MAGIC:
        raise SystemExit(f"bad magic {words[0]:#010x}: not an nRF update, or other keys")
    header = {
        "check": f"{words[1]:#010x}",  # meaning unknown (not a CRC32 / CRC16 / sum of the body)
        "word2": f"{words[2]:#x}",
        "address": words[3],
        "word4": f"{words[4]:#x}",  # 0x16000: room for the app?
        "size": words[5],  # the body is 2 bytes longer
        "header_len": words[6],
        "word7": f"{words[7]:#x}",
        "chip": text(h[0x2C:0x4C]),
        "firmware": text(h[0x4C:0x58]),
        "hardware": text(h[0x58:0x68]),
        "built": text(h[0x68:0x80]),
    }
    return header, xor(image[HEADER_LEN:], BODY_KEY)


def from_storage(path: Path) -> list[tuple[str, bytes]]:
    from littlefs import LittleFS

    data = path.read_bytes()
    fs = LittleFS(block_size=4096, block_count=len(data) // 4096, mount=False, name_max=255)
    fs.context.buffer = bytearray(data)
    fs.mount()
    found = []
    for root, _, files in fs.walk("/APP"):
        for name in files:
            if name.startswith("N51_") and name.endswith(".bin"):
                with fs.open(f"{root.rstrip('/')}/{name}", "rb") as f:
                    found.append((name, f.read()))
    return found


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("source", type=Path, help="storage partition image, or the .bin itself with --file")
    ap.add_argument("out", type=Path)
    ap.add_argument("--file", action="store_true", help="SOURCE is the encrypted N51_*.bin")
    a = ap.parse_args()
    images = [(a.source.name, a.source.read_bytes())] if a.file else from_storage(a.source)
    if not images:
        raise SystemExit("no /APP/*/N51_*.bin in this partition")
    a.out.mkdir(parents=True, exist_ok=True)
    for name, image in images:
        header, app = decode(image)
        sp, reset = struct.unpack_from("<2I", app)
        out = a.out / f"{Path(name).stem}_app_{header['address']:#x}.bin"
        out.write_bytes(app)
        print(f"{name}: {len(image)} bytes, {header}")
        print(f"  -> {out}: {len(app)} bytes, initial SP {sp:#010x}, reset {reset:#x}")


if __name__ == "__main__":
    main()
