# opencc600 — development and test process

Goal: test and validate **as automatically as possible**, including the behaviour on the **real CC600**,
not only on a PC. Context: [PRODUCT.md](PRODUCT.md) (crates), [REVERSE_NOTES.md](REVERSE_NOTES.md) (hardware).

## The rule that makes it possible

Everything except `firmware/` is `no_std` Rust with no hardware dependency (`nrf-link`, `gnss`, `altitude`,
`sensors`, `ride-store`, `fit`, `recorder`, `ui`, `companion`), so most of the code runs in `cargo test` on a PC.
`firmware/` only wires them to esp-hal and Embassy. Each layer below catches what the previous one cannot.

## 1. PC — `cargo test` at the repo root

- **Protocols** from byte fixtures: nRF frames, CRC16 and handshake (`nrf-link`, with `proptest`), NMEA and
  `$PAIR` (`gnss`, an anonymised capture in `crates/gnss/tests/fixtures/`), BLE GATT measurements and ANT+ pages
  (`sensors`), the Musette session with frames cut in BLE-sized writes (`companion`).
- **FIT output**: every file written by the tests is decoded again by `fitparser`, an independent decoder (header,
  CRCs, field numbers, scales). `tools/check_fit.py` does the same with the official Garmin SDK on real rides.
- **Power-cut fuzzing**: the RAM-backed flash "cuts the power" at **every** flash operation in turn, half-applied
  program / erase included, then the ride is recovered and checked (`ride-store/tests/power_cut.rs`,
  `recorder/tests/rides.rs`, recovery included). This is the test behind "a ride is never lost". Room management
  when the storage is full: `ride-store/tests/room.rs`.
- **Altitude and D+**: the Kalman filter on synthetic rides with the measured sensor noise
  (`altitude/tests/filter.rs`); ascent / descent counts on profiles that fool naive counters
  (`recorder/tests/climb.rs`). The vendor's algorithm, replayed on its own rides with
  `tools/reverse/vendor_dplus.py`, is the comparison point (see [ALTITUDE.md](ALTITUDE.md)).
- **Screens**: `cargo test -p ui --test screens` draws every screen with sample values and fails on a glyph missing
  from the fonts, a key label wider than its box or text out of the screen. The PNGs land in `target/tmp/screens/`
  (the README's screenshots come from there); `tools/compare_screens.py` puts them next to the clickable
  prototype's (`tools/prototype_shots.py`).
- **A ride along a route through the real UI**: `cargo run -p route-tool -- sim IN.gpx OUT_DIR` writes the screens
  as PNG.
- Fixtures must not leak real positions: synthetic or anonymised tracks only (`dump/` and `*.log` are git-ignored).

Host tests need `BINDGEN_EXTRA_CLANG_ARGS` for littlefs2-sys (see the Firmware section of the README); never leave
it set when building the firmware.

## 2. Device (HIL) — pytest over USB

The CC600 on USB-C, everything through that single cable (native USB-Serial/JTAG, eFuses open):

- **Flash** `ota_0` only, with `tools/flash_ota0.py`, which refuses any other offset (never the bootloader, the
  partition table or the eFuses). The vendor firmware stays in `ota_1` as a fallback.
- **USB test hook** in the normal build: keys (`key <0-2> <click|long>`; the keys are on the nRF, there is no GPIO
  to poke), `sound off` (tests run next to people), `fix` (positions in place of the receiver's), `route
  begin/data/end`, `set` / `get`, `storage fill`, `gnss cold`... (list in the README, `tools/serial_log.py`). The
  link loses ~5 % of the lines when the log is busy: the host numbers each command and resends it until the
  firmware logs `usb ack <seq>`; a repeated number runs once.
- **Framework**: pytest in `tests/hil/` (`uv run --project tests/hil pytest tests/hil`). The `dut` fixture resets
  the device and mutes it; `dut.press(1)`, `dut.send(...)` (acknowledged, resent), `dut.expect(regex)` on the log,
  `dut.read_storage()` (esptool read of the storage partition, mounted on the PC with littlefs-python; the device
  resets afterwards). Assertions go on what the device stored (FIT files checked with the Garmin FIT SDK) rather
  than on log lines, which can be lost.
- **Tests**: a ride with pauses and one interrupted by a reset (`test_ride`), a route ridden with positions sent over
  USB (`test_route`, `tools/replay_gpx.py`), BLE sensors against `tools/fake_sensor.py` on the PC
  (`test_ble_sensors`), ANT+ search and channels without a sensor (`test_ant_sensors`), the Musette link with the PC
  playing the app (`test_companion`), automatic power off (`test_auto_off`), a full storage
  (`test_storage_full`), a silent GNSS receiver powered on again (`test_gnss`). The tests that change the route or
  the pairings put them back afterwards.
- **Crash recovery of the ride store**: `tools/hil_storage.py` with the `hil-storage` firmware build (back-to-back
  appends, reset 1–8 s after each boot, every byte checked against a known pattern). First run, 2026-10-19: 30
  resets, 1057 appends, 11 rides closed, nothing lost. A real battery cut needs the case open: out of scope; a
  reset is not a brown-out.
- **GNSS**: a fix indoors in ~6 s by a window, enough for smoke tests; `tools/ttff.py` measures the time to first
  fix after forced restarts.

Pitfalls: the nRF stream stops on a real power cut and only a physical long press on key 0 restarts it, so tests
never really power off (`power off dry`).

## 3. Field

- Real rides, the trip log on when needed (`set trip_log on`: one line a second in `/logs/*.csv`), pulled with
  `tools/pull_rides.py` or Musette and compared with another bike computer.
- **Every real bug becomes a test** (PC if possible, HIL otherwise) before it is fixed.

## Not done yet

- **CI**: nothing runs automatically. The plan: `fmt`, `clippy`, host tests and a firmware build with a size check
  (≤ 0x420000) on every push; the HIL suite on a dedicated device on a self-hosted runner before each release.
- **Panic info kept on the device** (RTC memory or the vendor `coredump` partition), sent with the ride sync.
- Considered and set aside for now: QEMU (the i80 panel is not emulated), an SDL simulator (`route-tool sim` and the
  screens test cover the need), traces recorded from c606-oss (the drivers were checked on the device directly).
