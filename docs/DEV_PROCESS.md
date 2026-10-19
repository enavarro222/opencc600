# opencc600 — development and test process

Goal: test and validate **as automatically as possible**, including the behaviour on the **real CC600**,
not only on a PC. Context: [PRODUCT.md](PRODUCT.md) (stack, crates), [REVERSE_NOTES.md](REVERSE_NOTES.md) (hardware).

## The rule that makes it possible

Everything is `no_std` Rust behind traits. Only `cc600-board` touches esp-hal; `core`, `ui`, `fit`, `nrf-link`
and `gnss` never do. So ~90 % of the code runs in `cargo test` on a PC, and the same binary logic runs in the
simulator, in QEMU and on the device.

Each layer below catches what the previous one cannot.

## 1. PC — every commit, seconds

- **Unit tests**: maths (distance, averages, zones), route handling, corridor clipping, UTC/RTC.
- **Protocol tests**: nRF frames + CRC16 + handshake, NMEA / `$PAIR`, BLE GATT profiles (Heart Rate,
  Cycling Speed & Cadence, Cycling Power) — parsed from byte fixtures.
- **FIT output**: every FIT file written in tests is decoded again by an independent FIT decoder and checked.
- **Property tests** (`proptest`): random key and event sequences ("monkey"); invariants: no panic, the ride being
  recorded is never corrupted.
- **UX rules as tests** (from PRODUCT.md):
  - every state of the screen state machine shows a label for each of the 3 keys, or marks it inactive;
  - every transition flagged `irreversible` goes through a confirmation state.
  Checked by walking all states, not by hand.
- **Altitude / D+**: the filter is fed recorded or synthetic pressure + temperature series; checks: no D+ on a flat
  ride with sensor noise and slow weather drift, known D+ on a synthetic climb, temperature steps do not create
  fake climbs.
- **D+ against the vendor firmware**: the vendor FIT files already on the device (`/littlefs/FITS/FIT/`) carry
  per-record altitude and `total_ascent`. Their altitude series is replayed through our filter and the D+ must be
  within a small tolerance of the vendor's. Better: record a few rides with raw nRF pressure frames logged and the
  vendor FIT side by side (two units, or c606-oss with logging + the vendor on the same ride). Only the altitude /
  pressure / time columns become fixtures, no positions.
- **Power-cut fuzzing**: the storage fake (RAM-backed flash) "cuts the power" at **every** write in turn, then
  reboots the logic and checks that the ride is recovered. This is the test behind "a ride is never lost".
  *Exists*: `crates/ride-store/tests/power_cut.rs` (988 cuts, half-applied program / erase, checked against a
  deliberately non-atomic append).

## 2. Simulator — every commit

`sim` renders the real `ui` crate with `embedded-graphics-simulator`.

- **Golden images**: each screen in each state is rendered headless to PNG and compared with the committed
  reference (`insta`-style review). The PNGs double as the screen spec.
- **Scenario replay**: a scenario = GNSS NMEA stream + nRF frames (keys, battery) + sensor data + timed key
  presses, played faster than real time. Assertions on the FIT file, the totals and the screens.
  Cases: GNSS loss in a forest, sensor dropout, route changed mid-ride, power off then resume a ride.
- **Interactive mode** (SDL window, 3 keyboard keys) to try screens by hand.
- Scenario fixtures must not leak real home positions: synthetic or anonymised tracks only
  (`*.log` and dumps stay git-ignored).

## 3. Reference traces from c606-oss — once, then every commit

The case is never opened, so no logic analyzer: traces are recorded **from the ESP32 side**.
c606-oss is run on the CC600 with logging added on its UART2 (nRF), UART0 (GNSS) and LCD command writes;
the byte streams become fixtures. The Rust drivers, on `embedded-hal-mock` / fake UARTs, must produce the same
sequences (panel init, nRF handshake, `$PAIR` setup) and parse the recorded answers. This checks the rewrite
is faithful before flashing anything.

## 4. QEMU (Espressif fork, ESP32-S3) — every commit

Runs the **real firmware binary** for what the PC cannot show: Xtensa build, boot, RAM and stack limits,
Embassy tasks, flash writes. The nRF and GNSS UARTs are fed from scenario files through QEMU serial ports.
The LCD_CAM i80 panel is not emulated: the QEMU build uses a framebuffer backend instead, and its screenshots are
compared with the simulator's (a difference means a porting issue, not a logic issue).
*(To check: which esp-hal peripherals the Espressif QEMU fork covers well enough.)*

## 5. Hardware bench (HIL) — nightly and before each release

One CC600 dedicated to tests, on USB-C to a self-hosted CI runner. Everything goes through that single cable
(native USB-Serial/JTAG, eFuses open):

- **Flash** `ota_0` only, with our script that refuses any other offset (never bootloader, partition table, eFuses).
  The vendor firmware stays in `ota_1` as a fallback.
- **Logs** over USB serial (`defmt` / `esp-println`), **on-target tests** with `embedded-test` + probe-rs
  (built-in JTAG).
- **Test commands** over USB serial, only in test builds:
  - inject key events (the keys are on the nRF, so there is no GPIO to poke);
  - dump the framebuffer as PNG → compared with the simulator golden image;
  - load a route, start/stop a ride, read back a FIT file.
- **Crash recovery**: reset at random points during recording, check the ride at the next boot.
  *Exists for the ride store*: `tools/hil_storage.py` with the `hil-storage` firmware build (back-to-back appends,
  reset 1–8 s after each boot, every byte checked against a known pattern). First run, 2026-10-19: 30 resets,
  1057 appends, 11 rides closed, nothing lost. A real battery cut needs the case open → out of scope for now; a reset is not a brown-out.
- **Sensors**: a second board (ESP32 or nRF dev kit) scripted to act as BLE heart-rate / cadence / power sensors,
  so BLE is tested without a rider.
- **GNSS**: fix works indoors (~6 s), enough for smoke tests; real rides cover the rest.

## 6. Field

- Panic info and logs kept on the device (RTC memory / a flash slot, the vendor `coredump` partition exists),
  uploaded with the ride sync.
- **Every real bug becomes a scenario** (step 2) before it is fixed.

## CI summary

| When | What |
|---|---|
| Every push / PR | `fmt`, `clippy`, host tests + property tests, golden images, scenario replays, power-cut fuzzing, reference traces, target build with size check (≤ 0x420000), QEMU smoke run |
| Nightly | long property / monkey runs, full HIL suite on the bench |
| Release | full HIL suite + a real ride |

The simulator and the headless renderer are command-line tools, so a coding agent can also run a scenario,
look at the PNGs and iterate on a screen without a device.
