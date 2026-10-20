"""Automatic power off during a ride: indoors without a fix the device is still, so with `auto_off_min 1` it
announces the power off after 30 s and switches off at 1 min, the ride paused and kept; a key cancels it.

The power off itself is replaced by a restart of the ESP32 (`power off dry`): a real one stops the nRF's sensor
stream (time, barometer) until a long press of the power key, and the next tests could not start a ride."""
import time

import pytest


@pytest.fixture
def auto_off_1min(dut):
    dut.send("set auto_off_min 1")
    yield
    dut.send("set auto_off_min 20")


def test_a_key_cancels_the_power_off(dut, auto_off_1min):
    dut.to_home()
    dut.press(2)  # start
    dut.wait_screen("Ride")
    mark = dut.mark()
    dut.expect(r"auto off: announced", timeout=40, since=mark)
    dut.press(2)  # cancels it: not a pause
    dut.expect(r"auto off: not announced", timeout=3, since=mark)
    assert dut.screen() == "Ride"
    # the delay starts again: announced again 30 s later, not switched off meanwhile
    t = time.monotonic()
    dut.expect(r"auto off: announced", timeout=40, since=dut.mark())
    assert time.monotonic() - t >= 25
    assert not [line for line in dut.lines[mark:] if "auto off: still" in line.text]
    dut.press(2)  # cancel
    dut.press(2)  # pause
    dut.press(1)  # finish...
    dut.press(2)  # ...confirmed
    dut.wait_screen("Saved")


def test_switched_off_with_the_ride_kept(dut, auto_off_1min):
    dut.send("power off dry")
    dut.to_home()
    dut.press(2)  # start
    dut.wait_screen("Ride")
    mark = dut.mark()
    dut.expect(r"auto off: still for 1 min", timeout=75, since=mark)
    dut.expect(r"ride: paused", timeout=3, since=mark)
    dut.expect(r"kept for the next power-on", timeout=3, since=mark)
    dut.expect(r"power off: test hook", timeout=3, since=mark)
    time.sleep(2)  # restarting
    dut.send("sound off")
    dut.wait_screen("Resume", timeout=20)
    dut.press(1)  # finish...
    dut.press(2)  # ...confirmed
    dut.wait_screen("Saved")
