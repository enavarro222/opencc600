#!/usr/bin/env python3
"""Finds the BLE remotes (HID over GATT) around, pairs them with the PC one at a time and shows live what each
button sends, so each remote can be told apart and its buttons mapped for the CC600.

    python3 tools/ble_remote_probe.py        (asks for sudo: btmon, /dev/input and the HID descriptors need root)

What it keeps, in dump/ble-remotes/<date>/ (git-ignored):
  - hci.btsnoop       the whole radio exchange (pairing method, GATT, HID reports), `btmon -r` reads it back;
  - <addr>.rdesc      the HID report descriptor of each remote (the Report Map the CC600 will read);
  - events.log        every input event, with the remote and the time;
  - summary.txt       per remote: name, address, your description, the order you pressed the buttons in.

The remote's input devices are grabbed while it is tested: its keys do not reach the desktop.
At the end the remotes can be unpaired from the PC (asked).
"""

import fcntl
import glob
import os
import queue
import re
import select
import struct
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ANSI = re.compile(r"\x1b\[[0-9;]*m|\x01|\x02")
EVENT = struct.Struct("llHHi")  # struct input_event on 64-bit Linux
EVIOCGRAB = 0x40044590
EV_SYN, EV_KEY, EV_REL, EV_MSC = 0, 1, 2, 4
MSC_SCAN = 4
SKIP_BY_DEFAULT = re.compile(r"SHIELD|TV|Remote Control", re.I)


def key_names():
    names = {}
    try:
        with open("/usr/include/linux/input-event-codes.h") as f:
            for line in f:
                m = re.match(r"#define\s+((?:KEY|BTN)_\w+)\s+(0x[0-9a-fA-F]+|\d+)\s*$", line)
                if m:
                    names.setdefault(int(m.group(2), 0), m.group(1))
    except OSError:
        pass
    return names


KEYS = key_names()
REL = {0: "x", 1: "y", 6: "hwheel", 8: "wheel", 11: "wheel_hi", 12: "hwheel_hi"}


def btctl(*args, timeout=30):
    try:
        out = subprocess.run(
            ["bluetoothctl", *args],
            capture_output=True, text=True, timeout=timeout,
        ).stdout
    except subprocess.TimeoutExpired as e:
        out = e.stdout.decode() if isinstance(e.stdout, bytes) else (e.stdout or "")
    return ANSI.sub("", out)


def info(addr):
    out = btctl("info", addr, timeout=10)
    return {
        "name": (re.search(r"Name: (.*)", out) or [None, addr])[1].strip(),
        "paired": "Paired: yes" in out,
        "connected": "Connected: yes" in out,
        "hid": "00001812-" in out or "Appearance: 0x03c" in out,
        "raw": out,
    }


def scan(seconds=15):
    print(f"\nScan BLE ({seconds} s)... mets les télécommandes en mode appairage (souvent un appui long).")
    out = btctl("--timeout", str(seconds), "scan", "le", timeout=seconds + 10)
    addrs = []
    for m in re.finditer(r"Device ((?:[0-9A-F]{2}:){5}[0-9A-F]{2})", out):
        if m.group(1) not in addrs:
            addrs.append(m.group(1))
    found = []
    for a in addrs:
        i = info(a)
        if i["hid"] and not i["paired"]:
            found.append((a, i["name"]))
    return found


def ask(prompt, default=""):
    try:
        answer = input(prompt).strip()
    except EOFError:
        answer = ""
    return answer or default


def input_devices(addr):
    """The /dev/input/eventN of the remote (one per HID collection: keyboard, consumer keys, mouse...)."""
    out = []
    with open("/proc/bus/input/devices") as f:
        blocks = f.read().split("\n\n")
    for b in blocks:
        if f"Uniq={addr.lower()}" in b.lower():
            name = (re.search(r'N: Name="(.*)"', b) or [None, "?"])[1]
            for ev in re.findall(r"\bevent\d+\b", b):
                out.append((f"/dev/input/{ev}", name))
    return out


def save_descriptors(addr, outdir):
    saved = []
    for ue in glob.glob("/sys/class/hidraw/hidraw*/device/uevent"):
        with open(ue) as f:
            text = f.read()
        if f"HID_UNIQ={addr.lower()}" in text.lower():
            with open(os.path.join(os.path.dirname(ue), "report_descriptor"), "rb") as f:
                rdesc = f.read()
            path = os.path.join(outdir, addr.replace(":", "") + ".rdesc")
            with open(path, "wb") as f:
                f.write(rdesc)
            saved.append((path, len(rdesc)))
    return saved


class Session:
    """One interactive bluetoothctl kept open: its agent stays registered while pairing (a one-shot
    `bluetoothctl pair` quits before BlueZ asks the agent: "No agent available", pairing refused)."""

    def __init__(self):
        self.p = subprocess.Popen(
            ["bluetoothctl"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, bufsize=1,
        )
        self.lines = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.was_pairable = "Pairable: yes" in btctl("show", timeout=10)
        for c in ("agent NoInputNoOutput", "default-agent", "pairable on"):
            self.send(c)
        self.wait(r"Changing pairable on succeeded|pairable: yes", 5)

    def _read(self):
        for line in self.p.stdout:
            self.lines.put(ANSI.sub("", line))

    def send(self, cmd):
        self.p.stdin.write(cmd + "\n")
        self.p.stdin.flush()

    def wait(self, pattern, timeout):
        """The first line matching `pattern` before `timeout` s, else None."""
        end = time.time() + timeout
        while time.time() < end:
            try:
                line = self.lines.get(timeout=0.2)
            except queue.Empty:
                continue
            if re.search(r"Request confirmation|Authorize service|\(yes/no\)", line):
                self.send("yes")  # Just Works: accept
            if re.search(pattern, line, re.I):
                return line.strip()
        return None

    def close(self):
        if not self.was_pairable:
            self.send("pairable off")
            self.wait(r"pairable off succeeded|pairable: no", 3)
        self.send("quit")
        try:
            self.p.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.p.kill()


def pair(bt, addr, name):
    print(f"\n=== {name} ({addr}) ===")
    print("Appairage... regarde quelle télécommande change de voyant.")
    bt.send(f"pair {addr}")
    result = bt.wait(r"Pairing successful|Failed to pair.*", 40)
    if not info(addr)["paired"]:
        print(f"  pas appairée ({result or 'pas de réponse'}) : est-elle en mode appairage ?")
        return False
    print("  appairée")
    bt.send(f"trust {addr}")
    bt.wait(r"trust succeeded", 5)
    if not info(addr)["connected"]:
        bt.send(f"connect {addr}")
        bt.wait(r"Connection successful|Failed to connect.*", 20)
    return True


def record(addr, name, outdir, log):
    devices = []
    deadline = time.time() + 20
    while time.time() < deadline and not devices:
        devices = input_devices(addr)
        time.sleep(0.5)
    if not devices:
        print("  appairée mais aucun périphérique d'entrée n'est apparu (voir hci.btsnoop)")
        return
    time.sleep(1.0)  # every collection shows up
    devices = input_devices(addr)
    for path, size in save_descriptors(addr, outdir):
        print(f"  descripteur HID : {os.path.basename(path)} ({size} octets)")
    fds = {}
    for path, dname in devices:
        try:
            fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
            fcntl.ioctl(fd, EVIOCGRAB, 1)
            fds[fd] = dname
            print(f"  {path} : {dname}")
        except OSError as e:
            print(f"  {path} : {e}")
    print(
        f"\n>>> C'est {name}. Appuie sur chacun de ses boutons, un par un, lentement,\n"
        ">>> puis un appui long sur le bouton principal. Entrée (clavier du PC) quand c'est fini.\n"
    )
    log.write(f"\n# {name} {addr}\n")
    t0 = time.time()
    scan_code = {}
    moved = {}
    last_move = 0.0
    while True:
        ready, _, _ = select.select([sys.stdin, *fds], [], [])
        if sys.stdin in ready:
            sys.stdin.readline()
            break
        for fd in ready:
            try:
                data = os.read(fd, EVENT.size * 64)
            except OSError:
                print("  périphérique perdu (déconnectée ?)")
                fds.pop(fd, None)
                continue
            for off in range(0, len(data) - EVENT.size + 1, EVENT.size):
                _, _, etype, code, value = EVENT.unpack_from(data, off)
                t = time.time() - t0
                if etype != EV_SYN:
                    log.write(f"{t:8.3f} {fds[fd]!r} type={etype} code={code} value={value}\n")
                if etype == EV_MSC and code == MSC_SCAN:
                    scan_code[fd] = value
                elif etype == EV_KEY:
                    what = {0: "relâché", 1: "APPUI", 2: "répétition"}.get(value, value)
                    usage = scan_code.pop(fd, None)
                    u = f"  usage {usage >> 16:#04x}:{usage & 0xFFFF:#06x}" if usage is not None else ""
                    if value != 2:
                        print(f"  {t:7.2f} s  {KEYS.get(code, code):<18} {what}{u}")
                elif etype == EV_REL:
                    k = REL.get(code, code)
                    moved[k] = moved.get(k, 0) + value
                elif etype == EV_SYN and moved and t - last_move > 0.3:
                    print(f"  {t:7.2f} s  souris {' '.join(f'{k}={v:+d}' for k, v in moved.items())}")
                    moved.clear()
                    last_move = t
    for fd in fds:
        try:
            fcntl.ioctl(fd, EVIOCGRAB, 0)
        except OSError:
            pass
        os.close(fd)


def main():
    if os.geteuid() != 0:
        os.execvp("sudo", ["sudo", sys.executable, "-I", *sys.argv])
    outdir = os.path.join(ROOT, "dump", "ble-remotes", time.strftime("%Y%m%d-%H%M%S"))
    os.makedirs(outdir)
    btmon = subprocess.Popen(
        ["btmon", "-w", os.path.join(outdir, "hci.btsnoop")],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    tested = []
    bt = Session()
    try:
        with open(os.path.join(outdir, "events.log"), "w") as log, \
                open(os.path.join(outdir, "summary.txt"), "w") as summary:
            while True:
                found = scan()
                if not found:
                    print("Aucune télécommande BLE non appairée trouvée.")
                else:
                    print("\nTélécommandes trouvées :")
                    for n, (a, nm) in enumerate(found, 1):
                        mark = " *" if SKIP_BY_DEFAULT.search(nm) else ""
                        print(f"  {n}. {nm} ({a}){mark}")
                    default = " ".join(
                        str(n) for n, (_, nm) in enumerate(found, 1) if not SKIP_BY_DEFAULT.search(nm)
                    )
                    picks = ask(f"Numéros à tester [{default}] (* = pas par défaut) : ", default).split()
                    for p in picks:
                        if not p.isdigit() or not 1 <= int(p) <= len(found):
                            continue
                        addr, name = found[int(p) - 1]
                        if not pair(bt, addr, name):
                            summary.write(f"{name} {addr}: appairage raté\n")
                            continue
                        tested.append(addr)
                        record(addr, name, outdir, log)
                        desc = ask("Décris-la (forme, couleur, nombre de boutons) : ")
                        order = ask("Dans quel ordre as-tu appuyé ? (ex: haut, bas, OK, photo) : ")
                        summary.write(f"{name} {addr}\n  description: {desc}\n  ordre: {order}\n")
                        summary.flush()
                        log.flush()
                if ask("\nScanner encore (une autre télécommande) ? [o/N] ", "n").lower() not in ("o", "oui", "y"):
                    break
    finally:
        if tested and ask("\nDésappairer les télécommandes testées du PC ? [O/n] ", "o").lower() in ("o", "oui", "y"):
            for a in tested:
                bt.send(f"remove {a}")
                bt.wait(r"Device has been removed|not available", 10)
        bt.close()
        btmon.terminate()
        btmon.wait()
        uid, gid = os.environ.get("SUDO_UID"), os.environ.get("SUDO_GID")
        if uid and gid:
            for p in [outdir, *glob.glob(os.path.join(outdir, "*"))]:
                os.chown(p, int(uid), int(gid))
        print(f"\nRésultats dans {outdir}")


if __name__ == "__main__":
    main()
