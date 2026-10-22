"""The screen and the light: the night theme from sunset to sunrise at the device's position (`auto_night`), the
backlight from the ambient light sensor (`auto_backlight`), each back to its setting when switched off.

The positions come over USB (`fix`), at the device's clock: at the equator, the longitude where it is noon now and
the opposite one, where it is midnight (a margin of hours for a clock not set). The light level replaces the
sensor's (`light RAW`). `light ?` tells the theme and the backlight: polled, as log lines can be lost."""
import time
from datetime import datetime, timezone

import pytest


def noon_longitude() -> float:
    """Where the sun is highest now (to a few degrees: the equation of time is left out)."""
    now = datetime.now(timezone.utc)
    return ((12 - now.hour - now.minute / 60) * 15 + 180) % 360 - 180


NOON = noon_longitude()
MIDNIGHT = (NOON + 360) % 360 - 180


def get(dut, key: str) -> str:
    for _ in range(5):
        mark = dut.mark()
        dut.send(f"get {key}")
        try:
            return dut.expect(rf"usb: {key}=(\S+)", timeout=1.0, since=mark).group(1)
        except TimeoutError:
            continue
    raise TimeoutError(f"no value for {key}")


def light(dut) -> tuple[str, int]:
    for _ in range(5):
        mark = dut.mark()
        dut.send("light ?")
        try:
            m = dut.expect(r"light: theme (\w+), backlight (\d+) %", timeout=1.0, since=mark)
            return m.group(1), int(m.group(2))
        except TimeoutError:
            continue
    raise TimeoutError("no light line")


def wait_light(dut, theme=None, backlight=range(101), lon=None, timeout=10.0):
    """Waits for the theme and a backlight in `backlight`; at longitude `lon` (sent again, it holds 5 s)."""
    end = time.time() + timeout
    while True:
        if lon is not None:
            dut.send(f"fix 0 {round(lon * 1e7)}")
        shown, pct = light(dut)
        if theme in (None, shown) and pct in backlight:
            return
        if time.time() > end:
            raise AssertionError(f"theme {shown}, backlight {pct} % after {timeout} s")
        time.sleep(0.5)


@pytest.fixture
def settings_kept(dut):
    keys = ("theme", "auto_night", "backlight", "auto_backlight")
    before = {k: get(dut, k) for k in keys}
    yield
    dut.send("fix off")
    dut.send("light off")
    for k, v in before.items():
        dut.set(k, v)


def test_night_theme_from_sunset_to_sunrise(dut, settings_kept):
    dut.send("set theme day")
    dut.send("set auto_night on")
    wait_light(dut, theme="night", lon=MIDNIGHT)
    wait_light(dut, theme="day", lon=NOON)
    # switched off: the theme setting, wherever the device is
    dut.send("set theme night")
    dut.send("set auto_night off")
    wait_light(dut, theme="night", lon=NOON)
    dut.send("set theme day")
    wait_light(dut, theme="day", lon=MIDNIGHT)


def test_backlight_follows_the_light(dut, settings_kept):
    dut.send("set auto_backlight on")
    dut.send("light 400")  # covered
    wait_light(dut, backlight=[10], timeout=20)
    dut.send("light 2600")  # a torch, the sensor saturated
    wait_light(dut, backlight=[100], timeout=20)
    dut.send("light 1600")  # halfway: 55 %, to the step of 5 % near
    wait_light(dut, backlight=range(50, 61), timeout=20)
    # switched off: the backlight setting, whatever the light
    dut.send("set backlight 30")
    dut.send("set auto_backlight off")
    wait_light(dut, backlight=[30])
    dut.send("light 400")
    time.sleep(3)
    assert light(dut)[1] == 30
