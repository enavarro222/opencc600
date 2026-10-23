"""The firmware on an emulated CC600 (esp32sim, the fork with the CC600 board, as the demo): the same ELF as on the
device, run natively faster than real time, driven by an esp32sim action script. Its virtual BLE central plays the
companion app: frames of protocol-cc600 written with `ble write-cmd`, the device's answers logged as notifications.

Needs: esp32sim (the fork's release named by tools/demo/esp32sim.rev, fetched once; or built, `cargo build --release
-p esp32sim` in the fork, path in $ESP32SIM_BIN, default ../esp32sim/target/release/esp32sim), the S3 mask ROM
($ESP32SIM_ROM_DIR, or docs/demo/rom/ after `tools/demo/build.sh`), the firmware built (`cargo build --release` in
firmware/) and espflash for its image.
"""
import os
import re
import shutil
import struct
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ELF = ROOT / "firmware/target/xtensa-esp32s3-none-elf/release/opencc600"
PARTITIONS = ROOT / "tools/demo/partitions.csv"

# the companion service (firmware/src/ble.rs): the GATT table is ours, so its handles are stable; the discovery at
# the start of each run checks them
RX = 0x0009
TX = 0x000B
TX_CCCD = TX + 1
RX_UUID = "c039ed02-4557-4b35-bc20-5c9320871456"

HELLO, BEGIN, CHUNK, END = 0x01, 0x10, 0x11, 0x12
DEVICE_INFO, OK, ERROR = 0x81, 0xF0, 0xFF
ROUTE, MAP = 1, 2


def esp32sim() -> Path | None:
    """$ESP32SIM_BIN, a build next to this repository, or the fork's release named by tools/demo/esp32sim.rev (a
    `cc600-*` tag: fetched once and checked, tools/demo/fetch_esp32sim.py)."""
    p = Path(os.environ.get("ESP32SIM_BIN", ROOT.parent / "esp32sim/target/release/esp32sim"))
    if p.exists():
        return p
    rev = (ROOT / "tools/demo/esp32sim.rev").read_text().strip()
    if rev.startswith("cc600-"):
        out = subprocess.run([sys.executable, str(ROOT / "tools/demo/fetch_esp32sim.py"), rev, "native"],
                             capture_output=True, text=True)
        if out.returncode == 0:
            return Path(out.stdout.strip()) / "esp32sim"
    return None


def rom() -> Path | None:
    for d in [os.environ.get("ESP32SIM_ROM_DIR"), ROOT / "docs/demo/rom"]:
        if d and (Path(d) / "esp32s3_rev0_rom.elf").exists():
            return Path(d) / "esp32s3_rev0_rom.elf"
    return None


def flash_image(out: Path) -> Path:
    """The firmware as a clean flash image: bootloader, partition table, app, blank storage (tools/demo/build.sh)."""
    merged = out / "merged.bin"
    subprocess.run(["espflash", "save-image", "--chip", "esp32s3", "--flash-size", "16mb", "--merge",
                    "--partition-table", str(PARTITIONS), str(ELF), str(merged)], check=True, capture_output=True)
    data = merged.read_bytes().rstrip(b"\xff")
    image = out / "flash.bin"
    image.write_bytes(data + b"\xff" * (-len(data) % 4096))
    return image


def crc16(data: bytes) -> int:
    """CRC-16/CCITT-FALSE (protocol-cc600)."""
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def frame(op: int, seq: int, payload: bytes = b"") -> bytes:
    head = struct.pack("<BBBH", 1, op, seq, len(payload)) + payload
    return head + struct.pack("<H", crc16(head))


class Script:
    """An esp32sim action script: lines at emulated times, written in order."""

    def __init__(self, start: float):
        self.t = start
        self.lines: list[str] = []
        self.seq = 0

    def at(self, line: str, then: float = 0.1):
        self.lines.append(f"{self.t:.3f} {line}")
        self.t += then

    def write(self, op: int, payload: bytes = b"", then: float = 0.1):
        self.seq = (self.seq + 1) % 256
        self.at(f"ble write-cmd {RX} {frame(op, self.seq, payload).hex()}", then)

    def send_file(self, kind: int, data: bytes, chunk: int = 200, every: float = 0.03) -> None:
        """TransferBegin, the chunks, TransferEnd: a new file starts at 0, so the app needs no `TransferReady` and
        the writes are paced instead of following the acks (`every` per chunk, under the device's window)."""
        self.write(BEGIN, struct.pack("<BIH", kind, len(data), crc16(data)), then=0.3)
        for off in range(0, len(data), chunk):
            self.write(CHUNK, struct.pack("<I", off) + data[off:off + chunk], then=every)
        self.write(END, then=1.5)

    def text(self, stop_after: float = 3.0) -> str:
        return "\n".join(self.lines + [f"{self.t + stop_after:.3f} stop"]) + "\n"


@dataclass
class Run:
    log: str

    def lines(self, pattern: str) -> list[re.Match]:
        return [m for m in (re.search(pattern, line) for line in self.log.splitlines()) if m]

    def notifications(self) -> list[tuple[int, int, bytes]]:
        """Frames the device notified: (op, seq, payload)."""
        out = []
        for m in self.lines(rf"notification handle=0x{TX:04x} value=([0-9a-f]+)"):
            raw = bytes.fromhex(m[1])
            while len(raw) >= 7:
                n = 5 + struct.unpack_from("<H", raw, 3)[0] + 2
                f, raw = raw[:n], raw[n:]
                assert struct.unpack_from("<H", f, n - 2)[0] == crc16(f[:n - 2]), f"bad CRC from the device: {f.hex()}"
                out.append((f[1], f[2], f[5:n - 2]))
        return out


def run(script: Script, out: Path, image: Path, max_seconds: float, psram_mb: int = 2) -> Run:
    """Runs the firmware with `script`, until its `stop` (or `max_seconds` emulated), with the CC600's 2 MB of PSRAM
    unless told otherwise."""
    (out / "script.txt").write_text(script.text())
    cmd = [str(esp32sim()), "--chip", "s3", "--board", "cc600", "--boot", "rom", "--rom", str(rom()),
           "--flash-image", str(image), "--elf", str(ELF), "--flash-mb", "16", "--psram-mb", str(psram_mb), "--ble",
           "--script", str(out / "script.txt"), "--max-seconds", str(max_seconds), "--console", "usb", "--no-dump"]
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    log = p.stdout + p.stderr
    (out / "run.log").write_text(log)
    return Run(log)


def missing() -> str | None:
    """What this machine lacks to run the emulated device, if anything."""
    if not esp32sim():
        return "esp32sim not built (ESP32SIM_BIN)"
    if not rom():
        return "no S3 ROM (ESP32SIM_ROM_DIR, or tools/demo/build.sh)"
    if not ELF.exists():
        return "firmware not built (cargo build --release in firmware/)"
    if not shutil.which("espflash"):
        return "espflash not installed"
    return None
