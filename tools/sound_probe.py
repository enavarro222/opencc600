#!/usr/bin/env -S uv run -q --with pyserial --with numpy python3
"""Record the PC microphone while the firmware scans the nRF sound commands, then describe each sound.

Start this script, then long-press key 1 on the CC600: the bring-up firmware sends `E2 01 00..1B`, one every 3 s,
and logs `probe E2 01 XX` on the USB serial. Both streams are timestamped on the host clock, so every sound lands
in the window of the command that played it. For each window: silent, or the list of notes (frequency, duration).

Stay quiet during the scan (~90 s). Output in dump/sound_probe-<time>/ (audio.wav, serial.log, sounds.md).

usage: sound_probe.py [-p /dev/ttyACM0] [--max-seconds 150]
       sound_probe.py --analyse DIR      (analyse a previous recording again)
"""
import argparse
import re
import signal
import subprocess
import time
import wave
from pathlib import Path

import numpy as np

RATE = 48000
WINDOW = 3.0  # firmware SCAN_STEP
FRAME = 0.010  # analysis hop, s
# Piezo buzzer fundamentals, Hz. Above 7 kHz are the odd harmonics of the square-wave drive (3 kHz -> 9 kHz).
BAND = (1500, 7000)
# The audio recorder starts late, so sounds show up 0.15-0.35 s *before* their command in the serial log:
# each window is anchored on the first sound onset found this close to the command.
ANCHOR = (-0.6, 0.5)
PROBE = re.compile(r"probe E2 01 ([0-9A-F]{2})")
ANSI = re.compile(rb"\x1b\[[0-9;]*m")
DUMP = Path(__file__).resolve().parent.parent / "dump"


def record(port, max_seconds, out):
    import serial

    out.mkdir(parents=True)
    s = serial.Serial(port, 115200, timeout=0.2)
    audio = subprocess.Popen(
        ["pw-record", "--rate", str(RATE), "--channels", "1", "--format", "s16", str(out / "audio.wav")]
    )
    t0 = time.time()
    print("recording; long-press key 1 on the CC600 to start the scan")
    lines, buf, done_at = [], b"", None
    while time.time() - t0 < max_seconds and (done_at is None or time.time() < done_at):
        buf += s.read(4096)
        *chunk, buf = buf.split(b"\n")
        for raw in chunk:
            text = ANSI.sub(b"", raw).decode("utf-8", "replace").rstrip()
            line = f"{time.time() - t0:8.3f} {text}"
            lines.append(line)
            if "probe" in text or "scan" in text:
                print(line, flush=True)
            if "scan done" in text:
                done_at = time.time() + WINDOW
    audio.send_signal(signal.SIGINT)
    audio.wait()
    (out / "serial.log").write_text("\n".join(lines) + "\n")


def load(path):
    with wave.open(str(path)) as w:
        x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32)
        return x / 32768, w.getframerate()


def notes(x, rate, start, end, floor_db):
    """Notes in [start, end) s: runs of loud frames with a stable dominant frequency."""
    n = int(FRAME * rate * 2)  # 20 ms frames, 10 ms hop
    hop = int(FRAME * rate)
    freqs = np.fft.rfftfreq(n, 1 / rate)
    band = (freqs >= BAND[0]) & (freqs <= BAND[1])
    win = np.hanning(n)
    frames = []
    for i in range(int(start * rate), int(end * rate) - n, hop):
        if i < 0:
            continue
        spec = np.abs(np.fft.rfft(x[i : i + n] * win))[band]
        db = 20 * np.log10(spec.max() + 1e-9)
        frames.append((i / rate, db, freqs[band][spec.argmax()]))
    out, cur = [], None
    for t, db, f in frames:
        loud = db > floor_db + 15
        if loud and cur and abs(f - cur["f"][-1]) / cur["f"][-1] < 0.06:
            cur["f"].append(f)
            cur["end"] = t + FRAME
        else:
            if cur and cur["end"] - cur["start"] >= 0.03:
                out.append(cur)
            cur = {"start": t, "end": t + FRAME, "f": [f]} if loud else None
    if cur and cur["end"] - cur["start"] >= 0.03:
        out.append(cur)
    return [(n["start"], n["end"] - n["start"], float(np.median(n["f"]))) for n in out]


def analyse(out):
    x, rate = load(out / "audio.wav")
    probes = []
    for line in (out / "serial.log").read_text().splitlines():
        m = PROBE.search(line)
        if m:
            probes.append((float(line.split()[0]), int(m[1], 16)))
    levels = list(_levels(x, rate))
    floor_db = float(np.median([db for _, db in levels]))
    onsets = [t for (t, db), (_, prev) in zip(levels[1:], levels) if db > floor_db + 20 >= prev]
    anchors = [next((o for o in onsets if ANCHOR[0] <= o - t <= ANCHOR[1]), None) for t, _ in probes]
    rows = ["| `E2 01 XX` | notes (Hz × ms) | total |", "|---|---|---|"]
    for i, ((t, xx), start) in enumerate(zip(probes, anchors)):
        if start is None:
            rows.append(f"| {xx:02X} | silent | |")
            continue
        later = [a for a in anchors[i + 1 :] if a is not None]
        end = min([start + WINDOW] + later[:1]) - 0.02
        ns = notes(x, rate, start - 0.02, end, floor_db)
        desc = ", ".join(f"{f:.0f}×{d * 1000:.0f}" for _, d, f in ns) or "?"
        total = f"{(ns[-1][0] + ns[-1][1] - ns[0][0]) * 1000:.0f} ms" if ns else ""
        if ns and ns[-1][0] + ns[-1][1] >= end - 0.05:
            total += " (still sounding at the next command)"
        rows.append(f"| {xx:02X} | {desc} | {total} |")
    table = "\n".join(rows)
    (out / "sounds.md").write_text(table + "\n")
    print(table)


def _levels(x, rate):
    """(time, dB) of the strongest component in BAND, every 10 ms."""
    n, hop = int(FRAME * rate * 2), int(FRAME * rate)
    freqs = np.fft.rfftfreq(n, 1 / rate)
    band = (freqs >= BAND[0]) & (freqs <= BAND[1])
    win = np.hanning(n)
    for i in range(0, len(x) - n, hop):
        spec = np.abs(np.fft.rfft(x[i : i + n] * win))[band]
        yield i / rate, 20 * np.log10(spec.max() + 1e-9)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-p", "--port", default="/dev/ttyACM0")
    ap.add_argument("--max-seconds", type=float, default=150)
    ap.add_argument("--analyse", type=Path, help="analyse an existing recording directory")
    a = ap.parse_args()
    out = a.analyse
    if out is None:
        out = DUMP / time.strftime("sound_probe-%Y%m%d-%H%M%S")
        record(a.port, a.max_seconds, out)
    analyse(out)


if __name__ == "__main__":
    main()
