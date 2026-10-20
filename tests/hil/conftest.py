"""pytest fixtures for the hardware-in-the-loop tests (see dut.py).

Run (CC600 plugged in, normal firmware flashed in ota_0):
    uv run --project tests/hil pytest tests/hil [--port /dev/ttyACM0]
Logs and storage images go to dump/hil/ (they can contain real positions: dump/ is not in git).
"""
import os
import time
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


_stream_checked = False


def check_stream(d: Dut):
    """Once a session: the nRF's sensor stream (clock, barometer) runs. It stops on a real power cut (unplugged, a
    real power off) and only starts again on a long press of the power key: without it no ride starts, and the
    device shows its charging screen. Better to stop here than to fail every test."""
    global _stream_checked
    if _stream_checked:
        return
    time.sleep(3)  # the splash, then the screen picked
    if d.screen() == "Charging":
        pytest.exit("the nRF's sensor stream is off (charging screen): long press the top left key once", 2)
    _stream_checked = True


@pytest.fixture
def dut(request, out_dir):
    port = request.config.getoption("--port")
    if not Path(port).exists():
        pytest.skip(f"no CC600 on {port}")
    d = Dut(port, log_path=out_dir / f"{request.node.name}.log")
    d.reset()
    d.send("sound off")  # tests run next to people
    check_stream(d)
    yield d
    d.close()
