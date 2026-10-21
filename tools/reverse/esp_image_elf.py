#!/usr/bin/env python3
"""Turn an ESP32-S3 application image (an ota partition) into an ELF for a disassembler (Ghidra, objdump).

The app image is a header, then segments `{u32 load address, u32 length, data}`; each becomes a PT_LOAD
segment and a section at its address (DROM 0x3C.., IROM 0x42.., DRAM 0x3FC.., IRAM 0x403..), so the string and
literal-pool addresses resolve. No symbols: the ROM ones are added by tools/reverse/ghidra/AddRomSymbols.java.

usage: esp_image_elf.py APP.bin OUT.elf     (e.g. ota_1 cut out of dump/flash_full_16MB_*.bin)
"""
import struct
import sys

EM_XTENSA = 94
IMAGE_MAGIC = 0xE9


def segments(image: bytes):
    magic, count = image[0], image[1]
    if magic != IMAGE_MAGIC:
        sys.exit(f"not an ESP app image (magic {magic:#x})")
    entry = struct.unpack_from("<I", image, 4)[0]
    pos = 24  # header + extended header
    out = []
    for _ in range(count):
        addr, length = struct.unpack_from("<II", image, pos)
        out.append((addr, image[pos + 8 : pos + 8 + length]))
        pos += 8 + length
    return entry, out


def section_name(addr: int) -> str:
    if 0x3C000000 <= addr < 0x3E000000:
        return ".flash.rodata"
    if 0x42000000 <= addr < 0x44000000:
        return ".flash.text"
    if 0x40370000 <= addr < 0x403E0000:
        return ".iram"
    return ".dram"


def elf(entry: int, segs) -> bytes:
    names = b"\0.shstrtab\0" + b"".join(f"{section_name(a)}.{i}".encode() + b"\0" for i, (a, _) in enumerate(segs))
    ehsize, phsize, shsize = 52, 32, 40
    data_off = ehsize + phsize * len(segs)
    offsets, pos = [], data_off
    for _, data in segs:
        offsets.append(pos)
        pos += (len(data) + 3) & ~3
    shstr_off = pos
    sh_off = (shstr_off + len(names) + 3) & ~3
    n_sections = len(segs) + 2  # null, segments, .shstrtab
    out = bytearray()
    out += b"\x7fELF" + bytes([1, 1, 1]) + bytes(9)
    out += struct.pack("<HHIIIIIHHHHHH", 2, EM_XTENSA, 1, entry, ehsize, sh_off, 0x300, ehsize, phsize,
                       len(segs), shsize, n_sections, n_sections - 1)
    for (addr, data), off in zip(segs, offsets):
        text = section_name(addr) in (".flash.text", ".iram")
        flags = 5 if text else 6  # R+X / R+W
        out += struct.pack("<IIIIIIII", 1, off, addr, addr, len(data), len(data), flags, 4)
    for (_, data), off in zip(segs, offsets):
        out += bytes(off - len(out)) + data
    out += bytes(shstr_off - len(out)) + names
    out += bytes(sh_off - len(out))
    out += bytes(shsize)  # null section
    name_pos = len("\0.shstrtab\0")
    for i, ((addr, data), off) in enumerate(zip(segs, offsets)):
        name = f"{section_name(addr)}.{i}"
        text = section_name(addr) in (".flash.text", ".iram")
        flags = 0x2 | (0x4 if text else 0x1)  # ALLOC + EXECINSTR / WRITE
        out += struct.pack("<IIIIIIIIII", name_pos, 1, flags, addr, off, len(data), 0, 0, 4, 0)
        name_pos += len(name) + 1
    out += struct.pack("<IIIIIIIIII", 1, 3, 0, 0, shstr_off, len(names), 0, 0, 1, 0)
    return bytes(out)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    entry, segs = segments(open(sys.argv[1], "rb").read())
    for addr, data in segs:
        print(f"{section_name(addr):14} {addr:#010x} {len(data):#x}")
    open(sys.argv[2], "wb").write(elf(entry, segs))


if __name__ == "__main__":
    main()
