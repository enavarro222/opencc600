"""BLE sensors against a fake one on this PC (tools/fake_sensor.py, BlueZ): heart rate, speed and cadence, power,
all in one device. The device finds it in a scan, connects once it is paired (the settings), keeps the link, and
writes the values in the ride's records. The pairing is put back as it was at the end.

Needs a Bluetooth adapter on the PC (skipped without one) and `uv` for the fake sensor's dependencies."""
import re
import shutil
import subprocess
import time
from pathlib import Path

import pytest

from test_ride import decode, rides

ROOT = Path(__file__).resolve().parents[2]
ROLES = ("hr", "speed_cadence", "power")


def adapter() -> str | None:
    """The PC's Bluetooth address and kind, as the device names it (`ble:p:E0:E2:58:63:35:07`)."""
    if not shutil.which("bluetoothctl"):
        return None
    out = subprocess.run(["bluetoothctl", "show"], capture_output=True, text=True, timeout=10).stdout
    m = re.search(r"Controller ([0-9A-F:]{17}) \((public|random)\)", out)
    if not m or "Powered: yes" not in out:
        return None
    return f"ble:{m.group(2)[0]}:{m.group(1)}"


@pytest.fixture
def fake_sensor(out_dir):
    address = adapter()
    if not address:
        pytest.skip("no powered Bluetooth adapter on this PC")
    log = open(out_dir / "fake_sensor.log", "w")
    p = subprocess.Popen([ROOT / "tools" / "fake_sensor.py", "--seconds", "300"], stdout=log, stderr=subprocess.STDOUT)
    end = time.time() + 60  # the first run installs bless
    while "advertising" not in (out_dir / "fake_sensor.log").read_text():
        if p.poll() is not None or time.time() > end:
            p.kill()
            pytest.fail(f"fake sensor did not start: {(out_dir / 'fake_sensor.log').read_text()}")
        time.sleep(0.5)
    yield address
    p.terminate()
    p.wait(10)
    log.close()


@pytest.fixture
def pairing(dut):
    """The pairing before the test, put back after it."""
    before = {}
    for role in ROLES:
        mark = dut.mark()
        dut.send(f"get {role}")
        before[role] = dut.expect(rf"usb: {role}=(\S+)$", since=mark).group(1)
    yield
    dut.reset()  # a storage read leaves the device booting
    time.sleep(2)
    # read back: a line typed while the USB port comes back can arrive cut
    for role, value in before.items():
        for _ in range(5):
            try:
                dut.set(role, value)
            except TimeoutError:
                continue  # the line arrived cut: not saved
            mark = dut.mark()
            dut.send(f"get {role}")
            if dut.expect(rf"usb: {role}=(\S+)$", since=mark).group(1) == value:
                break
        else:
            raise AssertionError(f"{role} not put back to {value}")


def test_scan_finds_the_sensor(dut, fake_sensor):
    mark = dut.mark()
    dut.send("ble scan 10")
    dut.expect(rf"ble: found {re.escape(fake_sensor)} .*\"hr\"", timeout=15, since=mark)


def test_a_ride_records_the_sensors(dut, out_dir, fake_sensor, pairing):
    mark = dut.mark()
    for role in ROLES:
        dut.send(f"set {role} {fake_sensor}")
    for kind in ("HeartRate", "SpeedCadence", "Power"):
        dut.expect(rf"ble: subscribed {kind}$", timeout=30, since=mark)
    dut.to_home()
    before = set(rides(dut.read_storage(out_dir / "ble_before.bin")))
    # connected again after the reset, from the saved pairing
    mark = dut.mark()
    dut.reset()
    dut.send("sound off")
    dut.expect(r"ble: subscribed Power$", timeout=30, since=mark)
    dut.to_home()
    dut.press(2)  # start
    try:
        dut.wait_screen("Ride", timeout=5)
    except TimeoutError:
        pytest.fail("the ride did not start: no time yet? (the nRF silent: long press the top left key once)")
    # past BlueZ's 30 s ATT timeout: the link must hold (the device answers the PC's requests)
    time.sleep(45)
    dut.press(2)  # pause
    dut.finish_ride()
    time.sleep(1)
    lost = [line.text for line in dut.lines[mark:] if "ble: disconnected" in line.text]
    assert not lost, lost
    new = {k: v for k, v in rides(dut.read_storage(out_dir / "ble_after.bin")).items() if k not in before}
    assert len(new) == 1, new.keys()
    records = decode(next(iter(new.values())))["record_mesgs"]
    assert len(records) >= 40, len(records)
    # the fake sensor's ranges: 120..160 bpm, 85..95 rpm, 180..260 W (a few records may come before the first
    # notification)
    for field, lo, hi in (("heart_rate", 120, 160), ("cadence", 84, 96), ("power", 175, 265)):
        values = [r[field] for r in records if r.get(field) is not None]
        assert len(values) >= len(records) - 3, (field, len(values), len(records))
        assert all(lo <= v <= hi for v in values), (field, min(values), max(values))


def wait_selected(dut, want: list[str], timeout: float = 3.0):
    """Waits for the screen and its selection (a key is handled at the next turn of the main loop)."""
    end = time.time() + timeout
    while (got := dut.selected()) != want:
        if time.time() > end:
            raise AssertionError(f"{got} rather than {want}")
        time.sleep(0.2)


def test_pairing_from_the_menu(dut, fake_sensor, pairing):
    """Menu › Capteurs › Cardio: the scan lists the fake sensor first (the closest), picking it pairs it."""
    dut.send("set hr none")
    dut.to_home()
    dut.press(1)  # menu
    dut.wait_screen("Menu")
    for _ in range(3):
        if dut.selected() == ["Menu", "Sensors"]:
            break
        dut.press(1)
        time.sleep(0.5)
    wait_selected(dut, ["Menu", "Sensors"])
    dut.press(2)
    wait_selected(dut, ["Sensors", "HeartRate"])
    mark = dut.mark()
    dut.press(2)  # look for heart rate sensors
    wait_selected(dut, ["SensorScan", "HeartRate", "0"])
    dut.expect(rf"ble: found {re.escape(fake_sensor)} ", timeout=15, since=mark)
    time.sleep(2)  # on the screen, and the others heard meanwhile sorted
    dut.press(2)  # pick the first one
    dut.expect(rf"sensors: HeartRate = {re.escape(fake_sensor)}$", timeout=5, since=mark)
    wait_selected(dut, ["Sensors", "HeartRate"])
    dut.expect(r"ble: subscribed HeartRate$", timeout=30, since=mark)
    dut.expect(r'ble: .* is "OCC Fake"', timeout=5, since=mark)
    # back out: the scan has stopped, the link holds
    dut.press(0)
    dut.wait_screen("Menu")
    dut.press(0)
    dut.wait_screen("Home")
