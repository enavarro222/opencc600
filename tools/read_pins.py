#!/usr/bin/env python3
"""Read the live pin routing of a running ESP32-S3 over the built-in USB-JTAG.

Read-only: halts the CPUs ~1 s, dumps GPIO matrix / IO_MUX / GPIO state
registers, resumes. Prints, for each GPIO, the IO_MUX function, the
peripheral output signal routed to it and the peripheral inputs reading it.

usage: read_pins.py OPENOCD_DIR GPIO_SIG_MAP_H [--raw out.txt]
"""
import re
import subprocess
import sys
from pathlib import Path

GPIO = 0x60004000
IO_MUX = 0x60009000
UARTS = {0: 0x60000000, 1: 0x60010000, 2: 0x6002E000}
LCD_CAM = 0x60041000

READS = {
    "out_sel": (GPIO + 0x554, 49),
    "in_sel": (GPIO + 0x154, 256),
    "io_mux": (IO_MUX + 0x4, 49),
    "out": (GPIO + 0x4, 1),
    "out1": (GPIO + 0x10, 1),
    "enable": (GPIO + 0x20, 1),
    "enable1": (GPIO + 0x2C, 1),
    "in": (GPIO + 0x3C, 1),
    "in1": (GPIO + 0x40, 1),
    "lcd_cam": (LCD_CAM, 16),
}
for n, base in UARTS.items():
    READS[f"uart{n}_clkdiv"] = (base + 0x14, 1)
    READS[f"uart{n}_clkconf"] = (base + 0x78, 1)

# Signals that the IO_MUX routes directly (MCU_SEL != 1) on well known pads
DIRECT = {
    (19, 0): "USB_D-", (20, 0): "USB_D+",
    (43, 0): "U0TXD (iomux)", (44, 0): "U0RXD (iomux)",
}


def load_signals(path):
    # The header gives the input and the output signal of an index the same
    # number, so each index maps to all its names
    names = {}
    for name, idx in re.findall(r"#define\s+(\w+)_IDX\s+(\d+)", Path(path).read_text()):
        names.setdefault(int(idx), []).append(name)
    names = {k: "/".join(dict.fromkeys(v)) for k, v in names.items()}
    return names, names


def run_openocd(ocd_dir):
    cmds = ["init", "halt"]
    for addr, n in READS.values():
        cmds.append(f"mdw 0x{addr:08x} {n}")
    cmds += ["resume", "shutdown"]
    args = [f"{ocd_dir}/bin/openocd", "-s", f"{ocd_dir}/share/openocd/scripts",
            "-f", "board/esp32s3-builtin.cfg"]
    for c in cmds:
        args += ["-c", c]
    p = subprocess.run(args, capture_output=True, text=True, timeout=60)
    return p.stdout + p.stderr


def parse(log):
    mem = {}
    for line in log.splitlines():
        m = re.match(r"\s*0x([0-9a-f]{8}):((?:\s+[0-9a-f]{8})+)", line)
        if m:
            addr = int(m.group(1), 16)
            for i, w in enumerate(m.group(2).split()):
                mem[addr + 4 * i] = int(w, 16)
    return mem


def main():
    ocd_dir, sig_map = sys.argv[1], sys.argv[2]
    log = run_openocd(ocd_dir)
    if "--raw" in sys.argv:
        Path(sys.argv[sys.argv.index("--raw") + 1]).write_text(log)
    mem = parse(log)
    if GPIO + 0x554 not in mem:
        print(log)
        sys.exit("no register dump in OpenOCD output")
    ins, outs = load_signals(sig_map)

    def rd(name, i=0):
        return mem[READS[name][0] + 4 * i]

    out_v = rd("out") | rd("out1") << 32
    en_v = rd("enable") | rd("enable1") << 32
    in_v = rd("in") | rd("in1") << 32

    readers = {}
    for sig in range(256):
        v = rd("in_sel", sig)
        if v & 0x80:  # routed through the GPIO matrix
            g = v & 0x3F
            readers.setdefault(g, []).append(ins.get(sig, f"sig{sig}") + (" (inv)" if v & 0x40 else ""))

    print(f"{'GPIO':>4} {'mux':>3} {'pull':4} {'oe':2} {'out':3} {'in':2}  routing")
    for g in range(49):
        mux = rd("io_mux", g)
        fn = (mux >> 12) & 7
        pull = ("U" if mux & (1 << 8) else "") + ("D" if mux & (1 << 7) else "")
        osel = rd("out_sel", g) & 0x1FF
        oe, ov, iv = (en_v >> g) & 1, (out_v >> g) & 1, (in_v >> g) & 1
        parts = []
        if fn != 1:
            parts.append(DIRECT.get((g, fn), f"IO_MUX func{fn}"))
        elif osel == 256:
            if oe:
                parts.append(f"GPIO out={ov}")
        else:
            parts.append("-> " + outs.get(osel, f"sig{osel}"))
        if g in readers:
            parts.append("<- " + ", ".join(readers[g]))
        print(f"{g:>4} {fn:>3} {pull:4} {oe:>2} {ov:>3} {iv:>2}  {'; '.join(parts)}")

    print("\nUART dividers (baud ~= sclk / (div + frag/16)):")
    for n in UARTS:
        div = rd(f"uart{n}_clkdiv")
        conf = rd(f"uart{n}_clkconf")
        sel = (conf >> 20) & 3  # 1 = APB 80 MHz, 2 = RC_FAST, 3 = XTAL 40 MHz
        sclk = {1: 80e6, 3: 40e6}.get(sel)
        integ, frag = div & 0xFFF, (div >> 20) & 0xF
        sclk_div = ((conf >> 12) & 0xFF) + 1
        baud = sclk / sclk_div / (integ + frag / 16) if sclk and integ else None
        print(f"  UART{n}: clkdiv=0x{div:08x} clk_conf=0x{conf:08x} sclk_sel={sel}"
              + (f" -> ~{baud:.0f} baud" if baud else ""))
    print("\nLCD_CAM regs:", " ".join(f"{rd('lcd_cam', i):08x}" for i in range(16)))


if __name__ == "__main__":
    main()
