"""A ride along a route, the positions sent over USB in place of the receiver's (`fix`, tools/replay_gpx.py): the
route is received, the ride records them, its distance follows the speed. The route on the device before the test
is put back afterwards."""
import sys
import tempfile
import time
from pathlib import Path

from test_ride import decode, rides

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import replay_gpx  # noqa: E402
import send_route  # noqa: E402

KMH = 36.0
SEMICIRCLE_DEG = 180 / 2**31


def synthetic_gpx(path: Path):
    """2 km north then east, from a made-up place (no real position in the tests)."""
    lat0, lon0 = 45.0, 3.0
    pts = [(lat0 + i * 0.0009, lon0) for i in range(12)] + [(lat0 + 11 * 0.0009, lon0 + i * 0.0013) for i in range(1, 12)]
    trk = "".join(f'<trkpt lat="{la:.6f}" lon="{lo:.6f}"><ele>300</ele></trkpt>' for la, lo in pts)
    path.write_text(f"<gpx><trk><name>Test HIL</name><trkseg>{trk}</trkseg></trk></gpx>")


def test_ride_along_a_route(dut, out_dir):
    dut.reset()
    dut.to_home()
    fs = dut.read_storage(out_dir / "before.bin")
    before = set(rides(fs))
    kept_route = fs.open("/route", "rb").read() if "route" in fs.listdir("/") else None
    dut.reset()
    try:
        with tempfile.TemporaryDirectory() as d:
            gpx = Path(d) / "test.gpx"
            synthetic_gpx(gpx)
            assert send_route.send(dut, send_route.route_bytes(gpx)).startswith("loaded"), "route refused"
            points = replay_gpx.read_gpx(gpx)
        dut.send("sound off")
        dut.to_home()
        fixes = replay_gpx.fixes(points, kmh=KMH)
        sent = []
        start = time.monotonic()

        def ride(seconds):
            for _ in range(seconds):
                fix = next(fixes)
                dut.send(replay_gpx.command(fix))
                sent.append(fix)
                time.sleep(max(0.0, start + len(sent) - time.monotonic()))

        ride(5)  # a fix before the start
        dut.press(2)  # start
        ride(40)
        dut.press(2)  # pause
        dut.wait_screen("Paused")
        dut.press(1)  # finish...
        dut.wait_screen("ConfirmStop")
        dut.press(2)  # ...confirmed
        dut.wait_screen("Saved")
        dut.send("fix off")
        time.sleep(1)
        new = {k: v for k, v in rides(dut.read_storage(out_dir / "after.bin")).items() if k not in before}
    finally:
        dut.reset()
        if kept_route is not None:
            send_route.send(dut, kept_route)
        else:
            dut.send("route clear")

    assert len(new) == 1 and next(iter(new)).endswith(".fit"), new.keys()
    m = decode(next(iter(new.values())))
    records = [r for r in m["record_mesgs"] if "position_lat" in r]
    assert len(records) >= 35, len(records)
    # every recorded position is one that was sent
    sent_deg = [(lat / 1e7, lon / 1e7) for lat, lon, _, _ in sent]
    for r in records:
        p = (r["position_lat"] * SEMICIRCLE_DEG, r["position_long"] * SEMICIRCLE_DEG)
        nearest = min(replay_gpx.distance_m(p, q) for q in sent_deg)
        assert nearest < 2.0, (p, nearest)
    # the distance comes from the speed: 10 m/s while the timer ran
    s = m["session_mesgs"][0]
    expected = KMH / 3.6 * s["total_timer_time"]
    assert abs(s["total_distance"] - expected) <= 0.1 * expected, (s["total_distance"], expected)
