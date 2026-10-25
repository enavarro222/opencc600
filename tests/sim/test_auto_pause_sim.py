"""Auto-pause on the firmware: GNSS speed, settings, and manual pause precedence."""
import pytest

import sim

if why := sim.missing():
    pytest.skip(why, allow_module_level=True)


def test_auto_pause_and_manual_pause_on_the_device(tmp_path):
    script = sim.Script(start=5.0)
    script.at("serial set auto_off_min 0")
    script.at("serial fix 450000000 50000000 3000 0", then=0.5)
    script.at("serial key 2 click", then=1.0)

    def speed(value, seconds=7):
        for _ in range(seconds):
            script.at(f"serial fix 450000000 50000000 {value} 0", then=1.0)

    def screen():
        script.at("serial screen ?", then=0.2)

    speed(3000)
    speed(416)  # below the default 1.5 km/h
    screen()
    speed(417)  # at or above it
    screen()
    script.at("serial key 2 click", then=0.5)  # manual pause
    speed(8000)
    screen()  # still manually paused despite riding
    script.at("serial key 2 click", then=0.5)
    speed(8000)
    speed(0)
    script.at("serial set auto_pause off", then=0.5)
    speed(0)
    screen()  # disabling auto-pause releases it
    script.at("serial set auto_pause_kmh 3.6")
    script.at("serial set auto_pause on")
    speed(900)
    script.at("serial set auto_pause_kmh 0.1")
    speed(900)
    screen()
    run = sim.run(script, tmp_path, sim.flash_image(tmp_path), max_seconds=script.t + 5)
    assert not run.lines(r"PANIC|panicked"), "the firmware panicked: see run.log"
    assert [m[1] for m in run.lines(r"screen (\w+)")] == ["Ride", "Ride", "Paused", "Ride", "Ride"]
    assert [m[1] for m in run.lines(r"ride: auto-(paused|resumed)")] == [
        "paused", "resumed", "paused", "resumed", "paused", "resumed",
    ]
