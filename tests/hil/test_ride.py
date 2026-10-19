"""Ride recording, keys injected over USB. Indoors without a fix the time comes from the nRF RTC and the records
have no position, which is enough to check the recording, the pauses and the recovery."""
import time

from garmin_fit_sdk import Decoder, Stream


def rides(fs) -> dict[str, bytes]:
    if "rides" not in fs.listdir("/"):
        return {}
    return {name: fs.open(f"/rides/{name}", "rb").read() for name in fs.listdir("/rides")}


def decode(body: bytes) -> dict:
    assert Decoder(Stream.from_byte_array(bytearray(body))).check_integrity(), "FIT header or CRC"
    messages, errors = Decoder(Stream.from_byte_array(bytearray(body))).read()
    assert not errors, errors
    return messages


def test_ride_with_a_pause(dut, out_dir):
    before = set(rides(dut.read_storage(out_dir / "before.bin")))
    dut.reset()
    dut.send("sound off")
    dut.press(1)  # start
    time.sleep(30)
    dut.press(1)  # pause
    time.sleep(10)
    dut.press(1)  # resume
    time.sleep(20)
    dut.press(1, long=True)  # stop and save
    time.sleep(1)
    new = {k: v for k, v in rides(dut.read_storage(out_dir / "after.bin")).items() if k not in before}
    assert len(new) == 1 and next(iter(new)).endswith(".fit"), new.keys()
    m = decode(next(iter(new.values())))
    assert [e["event_type"] for e in m["event_mesgs"]] == ["start", "stop", "start", "stop_all"]
    s = m["session_mesgs"][0]
    assert abs(s["total_elapsed_time"] - 60) <= 2, s["total_elapsed_time"]
    assert abs(s["total_timer_time"] - 50) <= 2, s["total_timer_time"]
    # one record a second while the timer runs
    assert abs(len(m["record_mesgs"]) - s["total_timer_time"]) <= 2, len(m["record_mesgs"])


def test_interrupted_ride_is_closed_at_boot(dut, out_dir):
    before = set(rides(dut.read_storage(out_dir / "before.bin")))
    dut.reset()
    dut.send("sound off")
    dut.press(1)  # start, never stopped
    time.sleep(25)
    # the storage read resets the device mid-ride: the ride is still a .part in this image...
    new = [k for k in rides(dut.read_storage(out_dir / "cut.bin")) if k not in before]
    assert len(new) == 1 and new[0].endswith(".part"), new
    # ...and closed by the firmware at the boot that follows
    time.sleep(3)
    after = rides(dut.read_storage(out_dir / "closed.bin"))
    fit = new[0].replace(".part", ".fit")
    assert fit in after and new[0] not in after, sorted(after)
    m = decode(after[fit])
    assert [e["event_type"] for e in m["event_mesgs"]] == ["start", "stop_all"]
    # appends every 10 s: at most ~10 s of the 25 s lost
    assert len(m["record_mesgs"]) >= 12, len(m["record_mesgs"])
