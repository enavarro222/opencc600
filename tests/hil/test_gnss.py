"""The GNSS receiver switched off through the nRF (`gnss off`), as found at a boot on 2026-10-20: the firmware must
power it on again, and keep trying while it stays silent (that day a single power-on left a ride without GPS for
6 minutes, until the device was restarted)."""
import time


def test_a_receiver_switched_off_is_powered_on(dut):
    mark = dut.mark()
    dut.send("gnss off")
    dut.expect(r"gnss: no data for \d+ s .*powering it on", timeout=10, since=mark)
    dut.expect(r"gnss: receiver started", timeout=10, since=mark)


def test_the_power_on_is_retried_while_silent(dut):
    mark = dut.mark()
    dut.send("gnss off 1")  # the first power-on lost, as on 2026-10-20
    dut.expect(r"gnss: power-on dropped by the test hook", timeout=10, since=mark)
    retry = dut.mark()
    t0 = time.monotonic()
    dut.expect(r"gnss: no data for \d+ s .*powering it on", timeout=15, since=retry)
    assert time.monotonic() - t0 >= 5, "retried too soon"
    started = dut.mark()
    dut.expect(r"gnss: receiver started", timeout=10, since=started)


def start_and_erase(dut) -> int:
    """Starts a ride from the home screen and erases it (empty): its id, the Unix time it started at."""
    mark = dut.mark()
    dut.press(2)  # start
    started = int(dut.expect(r"ride (\d+): started", timeout=5, since=mark).group(1))
    dut.press(2)  # pause
    dut.wait_screen("Paused")
    dut.press(1)  # finish: an empty ride...
    dut.wait_screen("ConfirmEmpty")
    dut.press(2)  # ...erased
    dut.expect(rf"ride {started}: discarded", timeout=5, since=mark)
    dut.wait_screen("Home")
    return started


def test_a_silent_receiver_leaves_no_fix(dut):
    """The last fix of a receiver gone silent is not the position and the time for ever: the clock goes on (the RTC).
    A position sent over USB stands for the last fix, the receiver kept off by the test hook."""
    try:
        dut.to_home()
        dut.send("gnss off 100")  # silent until the next boot
        dut.send("fix 450000000 30000000")  # a made-up place
        first = start_and_erase(dut)
        time.sleep(10)  # past the hold of a position sent over USB (5 s) and the receiver's silence (3 s)
        second = start_and_erase(dut)
        assert second - first >= 10, f"the clock stopped at the last fix: ride {first}, then ride {second}"
    finally:
        dut.send("fix off")
        dut.reset()  # the receiver powered on again
