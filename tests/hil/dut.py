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

    def reopen(self, timeout: float = 30.0):
        """Opens the port again after the device went away (switched off, then started by the nRF on USB power)."""
        self._close()
        self._add("--- port closed by the test")
        end = time.time() + timeout
        while True:
            time.sleep(1)
            try:
                self._open()
                break
            except serial.SerialException:
                if time.time() > end:
                    raise
        self.seq = 0
        self._add("--- port opened again")

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

    def screen(self) -> str:
        """The screen shown (`crates/ui` `Screen`: Home, Ride, Paused, Resume, Saved...)."""
        for _ in range(5):
            mark = self.mark()
            self.send("screen ?")
            try:
                return self.expect(r"screen (\w+)( .*)?$", timeout=1.0, since=mark).group(1)
            except TimeoutError:
                continue  # the log line was lost
        raise TimeoutError("no screen line")

    def selected(self) -> list[str]:
        """The screen shown and what is selected on it: `["Menu", "Sensors"]`, `["Sensors", "Power"]`,
        `["SensorScan", "HeartRate", "0"]`."""
        for _ in range(5):
            mark = self.mark()
            self.send("screen ?")
            try:
                return self.expect(r"screen (\w+.*)$", timeout=1.0, since=mark).group(1).split()
            except TimeoutError:
                continue
        raise TimeoutError("no screen line")

    def wait_screen(self, name: str, timeout: float = 10.0):
        end = time.time() + timeout
        while (shown := self.screen()) != name:
            if time.time() > end:
                raise TimeoutError(f"screen {shown}, not {name}, after {timeout} s")
            time.sleep(0.5)

    def to_home(self):
        """Back to the home screen, finishing a ride left over by an earlier test (paused at power-on)."""
        for _ in range(6):
            shown = self.screen()
            if shown == "Home":
                return
            if shown == "Ride":
                self.press(2)  # pause
            elif shown in ("Paused", "Resume"):
                self.finish_ride()
            elif shown in ("Saved", "ConfirmStop"):
                self.press(2)
            elif shown == "ConfirmEmpty":
                self.press(1)  # saved all the same
            else:
                self.press(0)  # back
            time.sleep(0.5)
        raise RuntimeError(f"not back home: {self.screen()}")

    def finish_ride(self):
        """From the pause: finishes the ride and saves it. An empty ride (`ui::EMPTY_RIDE_S`, `ui::EMPTY_RIDE_CM`)
        asks otherwise: its main key erases it, the bottom left one saves it."""
        self.press(1)  # finish...
        end = time.time() + 5
        while (shown := self.screen()) not in ("ConfirmStop", "ConfirmEmpty"):
            if time.time() > end:
                raise TimeoutError(f"screen {shown}, not a confirmation")
            time.sleep(0.2)
        self.press(1 if shown == "ConfirmEmpty" else 2)  # ...saved

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
