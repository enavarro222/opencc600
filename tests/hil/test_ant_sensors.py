"""ANT+ through the nRF, without a sensor (no ANT+ stick on the PC to fake one): a search ends, and the channel of
a paired sensor away is opened and searched for. The pairing is put back as it was at the end (its channel closed).

On the tested CC600, the nRF acknowledges the open, reports search timeout (state 5) about 25 s later, then the
firmware opens the channel again. Some radio versions may not report the timeout; the firmware also retries those.

c606-oss found the nRF's ANT stack can go silent (until the three-key reset) when channels are opened in the first
seconds after boot or closed at boot: the firmware avoids both, and these tests do not restart the device while a
channel is open."""
import re

from test_ble_sensors import pairing  # noqa: F401 (fixture)

ABSENT = "ant:120:54321:1"  # a heart rate strap that is not there


def test_a_search_ends(dut):
    mark = dut.mark()
    dut.send("ant scan 5")
    dut.expect(r"ant: search done", timeout=15, since=mark)


def test_an_absent_sensor_is_looked_for_again(dut, pairing):  # noqa: F811
    mark = dut.mark()
    dut.send(f"set hr {ABSENT}")
    # opened at once (the device is up for more than 10 s)
    dut.expect(rf"ant: open {re.escape(ABSENT)}$", timeout=15, since=mark)
    dut.expect(r"nrf ack Frame\(A5 0C 6F F1 05 01 ", timeout=3, since=mark)
    # The CC600 reports search timeout (state 5) about 25 s after opening an absent sensor. The firmware must retry.
    dut.expect(r"ant: channel 120 .* state 5$", timeout=40, since=mark)
    retry_mark = dut.mark()
    dut.expect(rf"ant: open {re.escape(ABSENT)}$", timeout=10, since=retry_mark)
    dut.expect(r"nrf ack Frame\(A5 0C 6F F1 05 01 ", timeout=3, since=retry_mark)
    mark = dut.mark()
    dut.send("set hr none")
    dut.expect(r"ant: close type 120$", timeout=5, since=mark)
