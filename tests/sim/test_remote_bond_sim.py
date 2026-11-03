"""A remote's bond given to the BLE stack (as after its pairing, or at boot from /remote) while the device advertises
for the app: trouble-host syncs the controller's resolving list, which the S3 refuses while advertising (Command
Disallowed). trouble-host 0.7 then stopped the whole host: no app, no sensors, no remote until a reboot
(firmware/vendor/trouble-host). The app must still get its answers after it.

    uv run --project tests/hil pytest tests/sim
"""
import pytest

import sim

if why := sim.missing():
    pytest.skip(why, allow_module_level=True)

# the fields of a /remote bond line (crates/remote): LTK, EDIV, Rand, key size, level, identity, IRK
BOND = "0123456789abcdef0011223344556677 beef 0102030405060708 16 1 2A:07:98:2A:11:C6 public " \
       "00112233445566778899aabbccddeeff"

# the emulator refuses the sync while advertising, as an S3 does, from this release of the fork on
REFUSES_FROM = "cc600-2026.11.03"
OLD_EMULATOR = (sim.ROOT / "tools/demo/esp32sim.rev").read_text().strip() < REFUSES_FROM


def test_the_ble_host_outlives_a_refused_resolving_list_sync(tmp_path):
    script = sim.Script(start=3.0)  # the device advertises within a second
    # the sync waits for the advertising to stop: the device starts it again every 10 s (ADVERTISE_AGAIN in
    # firmware/src/ble/mod.rs), and the new advertising begins during the sync
    script.at(f"serial remote bond {BOND}", then=10.0)
    script.at("ble connect")
    script.at("ble mtu 247")
    script.at(f"ble subscribe {sim.TX_CCCD}", then=0.3)
    script.write(sim.HELLO, then=1.0)
    run = sim.run(script, tmp_path, sim.flash_image(tmp_path), max_seconds=script.t + 10)

    assert not run.lines(r"PANIC|panicked"), "the firmware panicked: see run.log"
    assert run.lines(r"usb: remote bond given to the stack")
    # the sync ran, and the controller refused it as an S3 does: what this test is about
    assert run.lines(r"\[host\] incremental resolving list add"), "no resolving list sync: see run.log"
    if OLD_EMULATOR and not run.lines(r"Command Disallowed"):
        pytest.skip(f"esp32sim before {REFUSES_FROM} takes the sync while advertising")
    assert run.lines(r"Command Disallowed"), "the controller took the sync: the test proves nothing"
    assert not run.lines(r"ble: host stopped"), "the BLE host stopped on the refused sync"
    assert any(op == sim.DEVICE_INFO for op, _, _ in run.notifications()), "the app got no answer after the bond"
