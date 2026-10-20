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
