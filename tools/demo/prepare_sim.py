#!/usr/bin/env python3
"""Apply the demo's CC600 ANT+ and power fixes without changing the shared BLE controller."""
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def prepare(checkout: Path) -> str:
    here = Path(__file__).resolve().parent
    patch = here / 'cc600-ant.patch'
    power = here / 'cc600-power.patch'
    source = here / 'cc600-ant.rs'
    target = checkout / 'esp32s3/src/board/cc600/ant.rs'
    if target.exists() and target.read_bytes() != source.read_bytes():
        raise ValueError(f'{target} already contains a different ANT+ implementation')
    command = ['git', '-C', str(checkout), 'apply']
    power_applied = subprocess.run([*command, '--reverse', '--check', str(power)], capture_output=True).returncode == 0
    if power_applied:
        # The lifecycle fix changes a hunk of the base extension. Validate the base on a temporary copy
        # with that fix reversed; never undo it in the developer's checkout just to check it.
        with tempfile.TemporaryDirectory() as temp:
            for relative in ('esp32s3/src/board/cc600.rs', 'esp32s3/src/board/cc600/nrf.rs', 'esp-soc/src/machine/web.rs'):
                destination = Path(temp) / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(checkout / relative, destination)
            subprocess.run(['git', '-C', temp, 'apply', '--reverse', str(power)], check=True)
            subprocess.run(['git', '-C', temp, 'apply', '--reverse', '--check', str(patch)], check=True)
        return hashlib.sha256(patch.read_bytes() + power.read_bytes() + source.read_bytes()).hexdigest()[:12]
    check = subprocess.run([*command, '--check', str(patch)], capture_output=True)
    if check.returncode == 0:
        subprocess.run([*command, str(patch)], check=True)
    else:
        # Rebuilding the same checkout is allowed; unrelated edits must never be overwritten.
        subprocess.run([*command, '--reverse', '--check', str(patch)], check=True)
    if not target.exists():
        target.write_bytes(source.read_bytes())
    subprocess.run([*command, '--check', str(power)], check=True)
    subprocess.run([*command, str(power)], check=True)
    return hashlib.sha256(patch.read_bytes() + power.read_bytes() + source.read_bytes()).hexdigest()[:12]


if __name__ == '__main__':
    print(prepare(Path(sys.argv[1])))
