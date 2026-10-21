"""A full storage: no ride starts (the home screen says why), a ride in progress says it is no longer recorded and is
written again once there is room, and the USB link keeps answering meanwhile.

The storage is filled with the test hook's `/fill.bin` (`storage fill N`: the logs in `/logs` deleted first, as a
ride short of room would, then until N blocks are left free), removed at the end. The rides on the device are kept:
a test deletes only the ride it recorded. Filling the whole partition takes a minute or two.
"""
import time

from test_ride import decode, rides


def fill(dut, left: int):
    m = dut.mark()
    dut.send(f"storage fill {left}")
    dut.expect(r"storage fill: done", timeout=300, since=m)


def unfill(dut):
    m = dut.mark()
    dut.send("storage unfill")
    dut.expect(r"storage fill: removed", timeout=10, since=m)


def ride(dut, seconds: float) -> int:
    """A ride of `seconds`, stopped and saved: its id."""
    m = dut.mark()
    dut.press(2)  # start
    started = int(dut.expect(r"ride (\d+): started", timeout=10, since=m).group(1))
    time.sleep(seconds)
    dut.press(2)  # pause
    dut.finish_ride()
    dut.expect(rf"ride {started}: saved", timeout=10, since=m)
    dut.press(2)  # the end screen's OK
    return started


def selected_ride(dut) -> int:
    for _ in range(5):
        m = dut.mark()
        dut.send("screen ?")
        try:
            return int(dut.expect(r"screen (?:Rides|ConfirmDelete) (\d+)$", timeout=1.0, since=m).group(1))
        except TimeoutError:
            continue
    raise TimeoutError("no rides screen line")


def trip_log(dut, on: bool | None = None) -> bool:
    """The `trip_log` setting, set first if `on` is given."""
    if on is not None:
        dut.send(f"set trip_log {'on' if on else 'off'}")
    m = dut.mark()
    dut.send("get trip_log")
    return dut.expect(r"usb: trip_log=(\w+)", timeout=2, since=m).group(1) == "on"


def test_a_full_storage_offers_to_delete_rides_then_starts(dut, out_dir):
    dut.to_home()
    # the trip log would write while the storage fills, and give that room to the ride
    was_on = trip_log(dut)
    trip_log(dut, False)
    mine = ride(dut, 12)
    try:
        fill(dut, 8)
        m = dut.mark()
        dut.press(2)  # start: refused, the rides screen instead
        dut.expect(r"ride: \d+ bytes free without the logs, not started", timeout=30, since=m)
        dut.wait_screen("Rides")
        # only the test's own ride is deleted: the newest, the last one listed
        for _ in range(100):
            if selected_ride(dut) == mine:
                break
            dut.press(1)  # next
        else:
            raise AssertionError(f"ride {mine} not listed")
        dut.press(2)  # delete...
        assert dut.screen() == "ConfirmDelete" and selected_ride(dut) == mine
        m = dut.mark()
        dut.press(2)  # ...confirmed
        dut.expect(rf"ride {mine}: deleted \(not sent\)", timeout=10, since=m)
        assert dut.screen() == "Rides"
        # room made (the filler goes): the screen offers to start
        unfill(dut)
        time.sleep(2)
        m = dut.mark()
        dut.press(2)  # start
        dut.expect(r"ride \d+: started", timeout=10, since=m)
        dut.wait_screen("Ride")
        dut.to_home()
    finally:
        unfill(dut)
        trip_log(dut, was_on)
    fs = dut.read_storage(out_dir / "after_delete.bin")
    assert f"{mine}.fit" not in rides(fs)


def test_a_nearly_full_storage_records_a_point_every_5_s(dut, out_dir):
    dut.to_home()
    # under 1 MB (256 blocks) for the rides, above the 64 KB a ride needs to start
    left = 200
    before = set(rides(dut.read_storage(out_dir / "before_sparse.bin")))
    dut.reset()
    dut.send("sound off")
    dut.to_home()
    try:
        fill(dut, left)
        m = dut.mark()
        dut.press(2)  # start
        dut.expect(r"ride: \d+ bytes left for the rides, a point every 5 s", timeout=10, since=m)
        time.sleep(40)
        dut.press(2)  # pause
        dut.finish_ride()
        time.sleep(1)
    finally:
        unfill(dut)
    new = {k: v for k, v in rides(dut.read_storage(out_dir / "after_sparse.bin")).items() if k not in before}
    assert len(new) == 1, new.keys()
    m = decode(next(iter(new.values())))
    times = [r["timestamp"].timestamp() for r in m["record_mesgs"]]
    gaps = {round(b - a) for a, b in zip(times, times[1:])}
    assert gaps == {5}, sorted(gaps)
    assert len(times) >= 6, len(times)


def test_a_ride_on_a_storage_that_fills_up(dut, out_dir):
    # a ride left over by an earlier run is saved by to_home(): read what is stored after it
    dut.to_home()
    before = set(rides(dut.read_storage(out_dir / "before_full.bin")))
    dut.reset()
    dut.send("sound off")
    dut.to_home()
    try:
        dut.press(2)  # start
        dut.wait_screen("Ride")
        time.sleep(12)
        # the first time, the logs (if any) give their room: filled again, nothing is left to delete. With one block
        # free a ride still appends (littlefs copies its last block there): it fails when it needs a new block, after
        # up to ~2 min at 34 bytes a second.
        for attempt in range(2):
            m = dut.mark()
            fill(dut, 0)
            try:
                dut.expect(r"ride: write failed, littlefs error -28", timeout=240, since=m)
                break
            except TimeoutError:
                if attempt == 1:
                    raise
        # nothing tried again for a minute: the USB link answers, the ride goes on
        t = time.time()
        assert dut.screen() == "Ride"
        assert time.time() - t < 5
        unfill(dut)
        dut.expect(r"ride: written again", timeout=90, since=m)
        time.sleep(12)
        dut.press(2)  # pause
        dut.finish_ride()
        time.sleep(1)
    finally:
        unfill(dut)
    new = {k: v for k, v in rides(dut.read_storage(out_dir / "after_full.bin")).items() if k not in before}
    assert len(new) == 1 and next(iter(new)).endswith(".fit"), new.keys()
    m = decode(next(iter(new.values())))
    # a valid file, with the records of before and after the full storage
    assert [e["event_type"] for e in m["event_mesgs"]] == ["start", "stop", "stop_all"]
    assert len(m["record_mesgs"]) >= 15, len(m["record_mesgs"])
