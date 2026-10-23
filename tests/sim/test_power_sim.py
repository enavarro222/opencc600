"""Power off and on with the USB plugged in, on the emulated device (sim.py): the nRF cuts the ESP32, the charger
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
    assert run.lines(r"INFO - power off(\x1b\[0m)?$")
    assert run.lines(r"chip reset .*cause 0x1\b"), "the charger did not start the ESP32 again"
    assert run.lines(r"charging screen: no sensor stream")
    assert run.lines(r"charging screen: switched on")
    assert screens(run) == ["Home", "Charging", "Home"]
