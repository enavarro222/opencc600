#!/usr/bin/env python3
"""Write an app image to the OTA slot the vendor firmware does not use, and nothing else; or put the vendor back.

Works on a CC600 or a CC700, whichever slot the vendor firmware runs from (it depends on the device's update
history: ota_0 until the first vendor OTA update, then the other slot at each update).

Safety rules (docs/REVERSE_NOTES.md): never eFuses, never the bootloader (0x0)
or the partition table (0x8000); only the free OTA slot (+ otadata), and the
storage partition only to erase it or put the vendor's back. The vendor
firmware stays in its slot as a fallback.

- reads the partition table from the device and refuses to go on unless it is
  the known CC600 / CC700 layout;
- keeps the backups of each device apart, in dump/<mac>/ (the eFuse MAC, e.g. dump/7c4fad84aea8/);
- saves the vendor otadata once to dump/<mac>/otadata.orig.bin (never overwritten);
- writes the image to the slot the vendor does not boot, then an otadata that boots it;
- when the storage partition holds the vendor's littlefs (v2.1, ours is v2.0), backs it up to
  dump/<mac>/storage.orig.bin and erases it: our firmware formats a blank partition at its next boot.

usage: flash.py --status [-p /dev/ttyACM0]      (slots, what they hold, which one boots, the storage)
       flash.py IMAGE.bin [-p /dev/ttyACM0] [--dry-run] [--keep-storage]
       flash.py --restore-vendor [-p /dev/ttyACM0]   (boot the vendor slot again)
       flash.py --restore-vendor --storage dump/<mac>/storage.orig.bin
                     (also write the vendor storage partition back: rides, settings, pairings)
       flash.py --erase-storage   (blank storage partition, whatever it holds, after the same backup)
"""
import argparse
import re
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from pathlib import Path

OTA = [(0x20000, 0x420000), (0x440000, 0x420000)]
OTADATA = (0xD000, 0x2000)
STORAGE = (0x860000, 0x79E000)
PART_TABLE = (0x8000, 0xC00)
ALLOWED_WRITES = {OTA[0][0], OTA[1][0], OTADATA[0], STORAGE[0]}

DUMP = Path(__file__).resolve().parent.parent / "dump"
# littlefs keeps its two superblocks in the first two blocks: enough to tell the backed up file system from another
SUPERBLOCKS = 0x2000
# littlefs on-disk versions in the superblock: the vendor formatted 2.1, the littlefs2 crate writes 2.0
LFS_VENDOR, LFS_OURS = 0x00020001, 0x00020000
# esp_app_desc_t: after the 24-byte image header and the first 8-byte segment header
APP_DESC = 24 + 8
APP_DESC_MAGIC = 0xABCD5432
OUR_PROJECT = "opencc600-firmware"
VENDOR_PROJECT = "Project"
# esp_ota_img_states_t
OTA_INVALID, OTA_ABORTED = 3, 4


def esptool(port, *args, after="hard-reset"):
    cmd = ["esptool", "--chip", "esp32s3", "-p", port, "--after", after, *args]
    print("+", " ".join(cmd))
    subprocess.run(cmd, check=True)


def write(port, offset, path, after="hard-reset"):
    if offset not in ALLOWED_WRITES:
        sys.exit(f"refusing to write at {offset:#x}")
    esptool(port, "write-flash", f"{offset:#x}", str(path), after=after)


def read_mac(port):
    r = subprocess.run(["esptool", "--chip", "esp32s3", "-p", port, "--after", "no-reset", "read-mac"],
                       capture_output=True, text=True)
    if r.returncode:
        sys.exit(f"esptool cannot reach the device on {port}:\n{r.stdout[-500:]}{r.stderr[-500:]}")
    out = r.stdout
    m = re.search(r"MAC:\s*([0-9a-f:]{17})", out)
    if not m:
        sys.exit(f"no MAC in the esptool output:\n{out}")
    return m.group(1).replace(":", "")


def read_flash(port, offset, size):
    with tempfile.TemporaryDirectory() as d:
        f = Path(d) / "region.bin"
        esptool(port, "read-flash", f"{offset:#x}", f"{size:#x}", str(f), after="no-reset")
        return f.read_bytes()


def read_partitions(port):
    raw = read_flash(port, *PART_TABLE)
    parts = {}
    for i in range(0, len(raw), 32):
        e = raw[i:i + 32]
        if e[:2] != b"\xaa\x50":
            break
        _, ptype, sub, off, size, name, _ = struct.unpack("<2sBBII16sI", e)
        parts[name.rstrip(b"\0").decode()] = (ptype, sub, off, size)
    return parts


def check_layout(parts):
    expected = {
        "ota_0": (0, 0x10, *OTA[0]),
        "ota_1": (0, 0x11, *OTA[1]),
        "otadata": (1, 0, *OTADATA),
        "storage": (1, 0x83, *STORAGE),
    }
    for name, want in expected.items():
        if parts.get(name) != want:
            sys.exit(f"unexpected {name} partition {parts.get(name)}, not the known CC600 / CC700 layout")


def app_info(header):
    """('empty' | 'ours' | 'vendor' | 'other' | 'unknown', version) of the image starting with these bytes."""
    if set(header) == {0xFF}:
        return "empty", ""
    if header[0] != 0xE9:
        return "unknown", ""
    magic, = struct.unpack("<I", header[APP_DESC:APP_DESC + 4])
    if magic != APP_DESC_MAGIC:
        return "unknown", ""
    version = header[APP_DESC + 16:APP_DESC + 48].split(b"\0")[0].decode(errors="replace")
    project = header[APP_DESC + 48:APP_DESC + 80].split(b"\0")[0].decode(errors="replace")
    kind = {OUR_PROJECT: "ours", VENDOR_PROJECT: "vendor"}.get(project, "other")
    return kind, f"{project} {version}" if kind == "other" else version


def booted_slot(otadata):
    """The slot the bootloader boots with this otadata: the valid entry with the highest seq, ota_0 if none."""
    best = None
    for sector in (0, 0x1000):
        seq, = struct.unpack("<I", otadata[sector:sector + 4])
        state, crc = struct.unpack("<II", otadata[sector + 24:sector + 32])
        if seq == 0xFFFFFFFF or state in (OTA_INVALID, OTA_ABORTED):
            continue
        if crc != zlib.crc32(struct.pack("<I", seq), 0xFFFFFFFF):
            continue
        best = seq if best is None else max(best, seq)
    return 0 if best is None else (best - 1) % 2


def otadata_for(slot):
    """An otadata whose single entry boots this slot (state undefined, booted as valid, as a vendor OTA leaves it)."""
    seq = slot + 1
    crc = zlib.crc32(struct.pack("<I", seq), 0xFFFFFFFF)
    entry = struct.pack("<I", seq) + b"\xff" * 20 + struct.pack("<II", 0xFFFFFFFF, crc)
    return entry + b"\xff" * (OTADATA[1] - len(entry))


def read_state(port):
    check_layout(read_partitions(port))
    otadata = read_flash(port, *OTADATA)
    slots = [app_info(read_flash(port, off, 0x100)) for off, _ in OTA]
    return otadata, slots, booted_slot(otadata)


def print_state(slots, boot):
    for i, (kind, version) in enumerate(slots):
        print(f"ota_{i}: {kind} {version}{'   <- boots' if i == boot else ''}")


def storage_kind(superblocks):
    """'blank' | 'ours' | 'vendor' | 'unknown', from the first two blocks of the storage partition."""
    if set(superblocks) == {0xFF}:
        return "blank"
    versions = set()
    for block in (0, 0x1000):
        sb = superblocks[block:block + 32]
        if sb[8:16] == b"littlefs":
            versions.add(struct.unpack("<I", sb[20:24])[0])
    if versions == {LFS_OURS}:
        return "ours"
    if versions == {LFS_VENDOR}:
        return "vendor"
    return "unknown"


def backup_storage(port, storage_orig, superblocks):
    """Makes dump/<mac>/storage.orig.bin hold what the partition holds now; an older backup is kept beside it."""
    if storage_orig.exists() and storage_orig.read_bytes()[:SUPERBLOCKS] == superblocks:
        print(f"storage backup up to date: {storage_orig}")
        return
    image = read_flash(port, *STORAGE)
    if image[:SUPERBLOCKS] != superblocks:
        sys.exit("the storage partition changed while it was read: try again")
    if storage_orig.exists():
        older = storage_orig.with_name(time.strftime("storage.%Y%m%d-%H%M%S.bin",
                                                     time.localtime(storage_orig.stat().st_mtime)))
        storage_orig.rename(older)
        print(f"older storage backup kept as {older}")
    storage_orig.parent.mkdir(parents=True, exist_ok=True)
    storage_orig.write_bytes(image)
    print(f"storage partition saved to {storage_orig}")


def erase_storage(port, storage_orig, after="hard-reset"):
    superblocks = read_flash(port, STORAGE[0], SUPERBLOCKS)
    if storage_kind(superblocks) == "blank":
        print("storage partition already blank")
        return
    backup_storage(port, storage_orig, superblocks)
    esptool(port, "erase-region", f"{STORAGE[0]:#x}", f"{STORAGE[1]:#x}", after=after)


def vendor_slot(otadata_orig, slots, boot):
    """The slot holding the vendor firmware we keep: the one the saved vendor otadata boots."""
    if otadata_orig.exists():
        slot = booted_slot(otadata_orig.read_bytes())
    elif slots[boot][0] == "vendor":
        slot = boot
    else:
        sys.exit(f"the device boots ota_{boot} ({slots[boot][0]}) and there is no {otadata_orig}: "
                 "restore the vendor otadata first")
    if slots[slot][0] != "vendor":
        sys.exit(f"the vendor otadata boots ota_{slot}, which holds {slots[slot][0]}: the vendor firmware is gone")
    return slot


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("image", nargs="?", type=Path)
    ap.add_argument("-p", "--port", default="/dev/ttyACM0")
    ap.add_argument("--status", action="store_true", help="show the slots and which one boots, write nothing")
    ap.add_argument("--dry-run", action="store_true", help="check only, write nothing")
    ap.add_argument("--restore-vendor", action="store_true", help="write the vendor otadata back (boots its slot)")
    ap.add_argument("--storage", type=Path, help="with --restore-vendor: storage partition image to write back")
    ap.add_argument("--erase-storage", action="store_true", help="back up then erase the storage partition")
    ap.add_argument("--keep-storage", action="store_true", help="with IMAGE: leave the vendor's storage partition as is")
    a = ap.parse_args()

    backups = DUMP / read_mac(a.port)
    otadata_orig = backups / "otadata.orig.bin"
    storage_orig = backups / "storage.orig.bin"
    print(f"backups of this device: {backups}")

    if a.status:
        _, slots, boot = read_state(a.port)
        print_state(slots, boot)
        print(f"storage: {storage_kind(read_flash(a.port, STORAGE[0], SUPERBLOCKS))}")
        esptool(a.port, "read-mac")  # hard reset: leave the ROM loader, boot again
        return

    if a.erase_storage:
        check_layout(read_partitions(a.port))
        erase_storage(a.port, storage_orig)
        return

    if a.restore_vendor:
        if not otadata_orig.exists():
            sys.exit(f"{otadata_orig} not found")
        _, slots, boot = read_state(a.port)
        vendor = vendor_slot(otadata_orig, slots, boot)
        print(f"vendor firmware in ota_{vendor} ({slots[vendor][1]})")
        if a.storage:
            img = a.storage.read_bytes()
            if len(img) != STORAGE[1] or img[8:16] != b"littlefs":
                sys.exit(f"{a.storage} is not a {STORAGE[1]:#x}-byte littlefs image")
            write(a.port, STORAGE[0], a.storage, after="no-reset")
        write(a.port, OTADATA[0], otadata_orig)
        return
    if a.storage:
        ap.error("--storage only goes with --restore-vendor")
    if not a.image:
        ap.error("IMAGE is required")

    image = a.image.read_bytes()
    if app_info(image[:0x100])[0] != "ours":
        sys.exit(f"{a.image} is not an {OUR_PROJECT} app image")
    if len(image) > OTA[0][1]:
        sys.exit(f"image is {len(image):#x} bytes, a slot holds {OTA[0][1]:#x}")

    otadata, slots, boot = read_state(a.port)
    print_state(slots, boot)
    vendor = vendor_slot(otadata_orig, slots, boot)
    target = 1 - vendor
    print(f"layout OK, vendor firmware in ota_{vendor}, image {len(image):#x} bytes -> ota_{target}")
    storage = storage_kind(read_flash(a.port, STORAGE[0], SUPERBLOCKS))
    clean_storage = storage in ("vendor", "unknown") and not a.keep_storage
    print(f"storage: {storage}{', to back up and erase' if clean_storage else ''}")

    if not otadata_orig.exists():
        backups.mkdir(parents=True, exist_ok=True)
        otadata_orig.write_bytes(otadata)
        print(f"vendor otadata saved to {otadata_orig}")

    if a.dry_run:
        esptool(a.port, "read-mac")  # hard reset: leave the ROM loader, boot again
        print("dry run: nothing written")
        return
    write(a.port, OTA[target][0], a.image, after="no-reset")
    with tempfile.TemporaryDirectory() as d:
        f = Path(d) / "otadata.bin"
        f.write_bytes(otadata_for(target))
        write(a.port, OTADATA[0], f, after="no-reset" if clean_storage else "hard-reset")
    if clean_storage:
        erase_storage(a.port, storage_orig)


if __name__ == "__main__":
    main()
