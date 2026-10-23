#!/usr/bin/env python3
"""Fetches a release of the esp32sim fork (its fork-release workflow): the WebAssembly emulator of the demo or the
native esp32sim of this platform, checked against the release's SHA256SUMS, unpacked into target/demo/esp32sim-<tag>/.
Prints the unpacked folder. Nothing is built here.

usage: tools/demo/fetch_esp32sim.py TAG wasm|native
"""
import hashlib
import platform
import re
import shutil
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

REPO = "https://github.com/enavarro222/esp32sim"
ROOT = Path(__file__).resolve().parents[2]


def platform_name() -> str:
    arch = {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "arm64", "arm64": "arm64"}[platform.machine().lower()]
    system = {"Linux": "linux", "Darwin": "macos"}[platform.system()]
    if (system, arch) == ("macos", "x86_64"):
        sys.exit("no esp32sim release for an Intel Mac: build it (ESP32SIM=<checkout>)")
    return f"{system}-{arch}"


def fetch(tag: str, kind: str) -> Path:
    if not re.fullmatch(r"cc600-[A-Za-z0-9._-]+", tag):
        sys.exit(f"not a release tag of the fork: {tag!r}")
    if not hasattr(tarfile, "data_filter"):
        sys.exit("Python 3.12 (or 3.11.4) at least: the archive is unpacked with tarfile's data filter")
    name = f"esp32sim-{tag}-{'wasm' if kind == 'wasm' else platform_name()}"
    out = ROOT / "target" / "demo" / f"esp32sim-{tag}"
    if (out / name).is_dir():
        return out / name
    out.mkdir(parents=True, exist_ok=True)
    base = f"{REPO}/releases/download/{tag}"
    sums = urllib.request.urlopen(f"{base}/esp32sim-{tag}-SHA256SUMS.txt").read().decode()
    want = next((line.split()[0] for line in sums.splitlines() if line.endswith(f"{name}.tar.gz")), None)
    if not want:
        sys.exit(f"{name}.tar.gz is not in the release {tag}")
    data = urllib.request.urlopen(f"{base}/{name}.tar.gz").read()
    if hashlib.sha256(data).hexdigest() != want:
        sys.exit(f"{name}.tar.gz does not match its SHA-256 in the release: not unpacked")
    # unpacked aside, then moved in place: a run cut halfway leaves nothing that looks complete
    with tempfile.TemporaryDirectory(dir=out) as tmp:
        archive = Path(tmp) / f"{name}.tar.gz"
        archive.write_bytes(data)
        with tarfile.open(archive) as t:
            t.extractall(tmp, filter="data")
        if not (Path(tmp) / name).is_dir():
            sys.exit(f"{name}.tar.gz does not hold {name}/")
        shutil.move(str(Path(tmp) / name), out / name)
    return out / name


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[2] not in ("wasm", "native"):
        sys.exit(__doc__)
    print(fetch(sys.argv[1], sys.argv[2]))
