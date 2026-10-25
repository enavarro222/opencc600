"""Power off and on, on battery and USB, on the emulated device (sim.py): the nRF cuts the ESP32, the charger
starts it again at once, without the nRF's sensor stream (the charging screen), and the long press of the power key
switches on.

    uv run --project tests/hil pytest tests/sim
"""
import pytest

import sim

if why := sim.missing():
    pytest.skip(why, allow_module_level=True)


def screens(run: sim.Run) -> list[str]:
    """What `screen ?` answered, in order."""
    return [m[1] for m in run.lines(r"screen (\w+)")]


def test_power_off_then_on_with_usb_plugged_in(tmp_path):
    script = sim.Script(start=0.0)
    script.at("board usb on")
    script.at("board battery 100 4300", then=5.0)
    script.at("serial screen ?", then=1.0)
    # the power off: a long press on the home screen
    script.at("press key0 1500", then=7.0)
    script.at("serial screen ?", then=1.0)
    # the power on, from the charging screen
    script.at("press key0 1500", then=3.0)
    script.at("serial screen ?", then=1.0)
    run = sim.run(script, tmp_path, sim.flash_image(tmp_path), max_seconds=script.t + 10)

    assert not run.lines(r"PANIC|panicked"), "the firmware panicked: see run.log"
    assert not run.lines(r"storage: littlefs error"), "storage did not survive the power cycle: see run.log"
    assert len(run.lines(r"storage: blank partition, formatted")) == 1, "the existing storage must not be reformatted"
    assert run.lines(r"INFO - power off(\x1b\[0m)?$")
    assert run.lines(r"chip reset .*cause 0x1\b"), "the charger did not start the ESP32 again"
    assert run.lines(r"charging screen: no sensor stream")
    assert run.lines(r"charging screen: switched on")
    assert screens(run) == ["Home", "Charging", "Home"]


def test_power_off_on_battery_stops_the_firmware_until_the_next_long_press(tmp_path):
    script = sim.Script(start=0.0)
    script.at("board usb off", then=5.0)
    script.at("serial screen ?", then=1.0)
    script.at("press key0 1500", then=5.0)
    # A switched-off ESP32 must not answer this command or continue its main loop.
    script.at("serial screen ?", then=2.0)
    script.at("press key0 1500", then=5.0)
    script.at("serial screen ?", then=1.0)
    run = sim.run(script, tmp_path, sim.flash_image(tmp_path), max_seconds=script.t + 5)

    assert not run.lines(r"PANIC|panicked"), "the firmware panicked: see run.log"
    assert not run.lines(r"storage: littlefs error"), "storage did not survive the power cycle: see run.log"
    assert len(run.lines(r"storage: blank partition, formatted")) == 1, "the existing storage must not be reformatted"
    assert len(run.lines(r"INFO - power off(\x1b\[0m)?$")) == 1
    assert not run.lines(r"still powered 2 s after power_off")
    assert len(run.lines(r"chip reset .*cause 0x1\b")) == 1
    assert screens(run) == ["Home", "Home"]
