"""pytest fixtures for the hardware-in-the-loop tests (see dut.py).

Run (CC600 plugged in, normal firmware flashed in ota_0):
    uv run --project tests/hil pytest tests/hil [--port /dev/ttyACM0]
Logs and storage images go to dump/hil/ (they can contain real positions: dump/ is not in git).
"""
import os
from pathlib import Path

import pytest

from dut import Dut

OUT = Path(__file__).resolve().parents[2] / "dump" / "hil"


def pytest_addoption(parser):
    parser.addoption("--port", default=os.environ.get("CC600_PORT", "/dev/ttyACM0"))


@pytest.fixture(scope="session")
def out_dir():
    OUT.mkdir(parents=True, exist_ok=True)
    return OUT


@pytest.fixture
def dut(request, out_dir):
    port = request.config.getoption("--port")
    if not Path(port).exists():
        pytest.skip(f"no CC600 on {port}")
    d = Dut(port, log_path=out_dir / f"{request.node.name}.log")
    d.reset()
    d.send("sound off")  # tests run next to people
    yield d
    d.close()
