"""The CC600 under test, over its USB serial port (ESP32-S3 USB-Serial/JTAG).

- the log is read by a background thread into timestamped lines; `expect` waits for a line matching a regex;
- `send` drives the firmware's USB test hook (`key 1 click`, `sound off`...). The link loses some lines in both
  directions when the log is busy, so each command carries a sequence number and is resent until the firmware
  logs `usb ack <seq>`; the firmware runs a repeated number once;
- `read_storage` reads the storage partition (read only, esptool) and mounts our littlefs on the PC. esptool
  resets the device afterwards, which also interrupts whatever it was doing.

Log lines can be lost: assert on what the device stores (rides, files) rather than on log lines when possible.
"""
import re
import subprocess
import threading
import time
from dataclasses import dataclass

import serial

ANSI = re.compile(rb"\x1b\[[0-9;]*m")
STORAGE = (0x860000, 0x79E000)


@dataclass
class Line:
    t: float
    text: str


class Dut:
    def __init__(self, port: str, log_path=None):
        self.port = port
        self.lines: list[Line] = []
        self.seq = 0
        self.t0 = time.time()
        self.log = open(log_path, "w") if log_path else None
        self._lock = threading.Lock()
        self._open()

    # --- serial link -------------------------------------------------------------------------------------------

    def _open(self):
        self.ser = serial.Serial(self.port, 115200, timeout=0.05)
        self._stop = False
        self._thread = threading.Thread(target=self._read, daemon=True)
        self._thread.start()

    def _close(self):
        self._stop = True
        self._thread.join()
        self.ser.close()

    def _read(self):
        buf = b""
        while not self._stop:
            try:
                buf += self.ser.read(4096)
            except serial.SerialException:
                time.sleep(0.1)  # re-enumeration after a reset
                continue
            *done, buf = buf.split(b"\n")
            for raw in done:
                self._add(ANSI.sub(b"", raw).decode("utf-8", "replace").rstrip())

    def _add(self, text: str):
        line = Line(time.time() - self.t0, text)
        with self._lock:
            self.lines.append(line)
        if self.log:
            self.log.write(f"{line.t:8.3f} {text}\n")
            self.log.flush()

    def close(self):
        self._close()
        if self.log:
            self.log.close()

    # --- device control ----------------------------------------------------------------------------------------

    def reset(self, boot_wait: float = 2.0):
        """Resets the ESP32 (RTS pulse, like esptool's hard reset) and waits for the firmware to run."""
        self.ser.dtr = False
        self.ser.rts = True
        time.sleep(0.1)
        self.ser.rts = False
        self.seq = 0  # the firmware forgets the last sequence number at boot
        self._add("--- reset by the test")
        time.sleep(boot_wait)

    def mark(self) -> int:
        """Position in the log, for `expect(..., since=mark)`."""
        with self._lock:
            return len(self.lines)

    def expect(self, pattern: str, timeout: float = 5.0, since: int = 0) -> re.Match:
        """Waits for a log line matching `pattern` (searched from line `since`)."""
        rx = re.compile(pattern)
        end = time.time() + timeout
        i = since
        while True:
            with self._lock:
                new = self.lines[i:]
            for line in new:
                i += 1
                if m := rx.search(line.text):
                    return m
            if time.time() > end:
                raise TimeoutError(f"no log line matching {pattern!r} within {timeout} s")
            time.sleep(0.05)

    def send(self, command: str, retries: int = 10, wait: float = 0.5):
        """Runs `command` on the firmware's USB test hook, resent until acknowledged."""
        self.seq += 1
        seq = self.seq
        for _ in range(retries):
            mark = self.mark()
            self._add(f">>> {seq} {command}")
            self.ser.write(f"{seq} {command}\r\n".encode())
            try:
                self.expect(rf"usb ack {seq}$", timeout=wait, since=mark)
                return
            except TimeoutError:
                continue
        raise TimeoutError(f"command {command!r} not acknowledged after {retries} tries")

    def press(self, key: int, long: bool = False):
        self.send(f"key {key} {'long' if long else 'click'}")

    # --- storage -----------------------------------------------------------------------------------------------

    def read_storage(self, image_path) -> "littlefs.LittleFS":  # noqa: F821
        """Reads the storage partition and mounts it; the device is reset afterwards (esptool hard reset)."""
        from littlefs import LittleFS

        self._close()
        try:
            subprocess.run(
                ["esptool", "--chip", "esp32s3", "-p", self.port, "read-flash", hex(STORAGE[0]), hex(STORAGE[1]),
                 str(image_path)],
                check=True, stdout=subprocess.DEVNULL,
            )
        finally:
            time.sleep(1.0)  # re-enumeration
            self._open()
            self.seq = 0
            self._add("--- reset by esptool (storage read)")
        data = open(image_path, "rb").read()
        fs = LittleFS(block_size=4096, block_count=len(data) // 4096, mount=False, name_max=255)
        fs.context.buffer = bytearray(data)
        fs.mount()
        return fs
