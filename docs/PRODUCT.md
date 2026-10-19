# opencc600 — product definition and plan

Hardware and reverse engineering background: [REVERSE_NOTES.md](REVERSE_NOTES.md).

## Decisions

### 2026-10-19, evening (replaces the morning's software stack)
- **Language and stack**: **Rust, `no_std`**, on [`esp-hal`](https://github.com/esp-rs/esp-hal) + Embassy (async).
  No ESP-IDF, no LVGL. The `no_std` core compiles unchanged on a PC, which is what makes most of the testing
  automatic (see [DEV_PROCESS.md](DEV_PROCESS.md)).
- **c606-oss is rewritten, not reused**: it becomes the **reference** for a Rust hardware layer (ST7789 i80 init,
  nRF UART protocol, GNSS commands, FIT writer), together with its `docs/HARDWARE.md`. Both projects are
  GPL-3.0, credits kept.
- **Radio stays in Rust**: BLE with `esp-radio` + `trouble-host`, Wi-Fi with `esp-radio` + `embassy-net`.
  The radio blobs are Espressif's whatever the language; the risk is kept behind traits so a brick can be
  swapped. ANT+ stays on the nRF and is driven over the UART link.
- **Testing**: as automatic as possible, **including on the real hardware**, not only on a PC.
  See [DEV_PROCESS.md](DEV_PROCESS.md).

### 2026-10-19, morning (still valid)
- c606-oss is a **hardware reference**, not the application: its UI/app is not what we want.
- **License**: GPL-3.0-or-later.
- **Hardware target**: CC600 = Magene C606 board **without eMMC and without touch** (pinout checked over JTAG).

## What we port from c606-oss
| Block | c606-oss reference | Status on the CC600 (C) | Rust target |
|---|---|---|---|
| ST7789 panel, 16-bit i80 bus, backlight | `main/lcd.c`, `main/backlight.c`, `main/board_c606.h`, `main/board.h` | OK | `cc600-board` (esp-hal LCD_CAM i8080 + LEDC) + `embedded-graphics` |
| nRF link (UART2, CRC16 frames, handshake, keys, battery, temperature/pressure, RTC) | `main/nrf_link.c` | OK | `nrf-link`: pure protocol (`no_std`, host tested) + UART glue |
| ANT+ sensors via the nRF | `main/ant.c`, `main/sensor_list.c` | not tested yet | `nrf-link` (ANT+ messages) |
| Airoha AG3352Q GNSS (UART0 921600, NMEA, `$PAIR`) | `main/gps.c` | OK, fix in ~6 s | `gnss`: NMEA (`nmea` crate) + `$PAIR` commands |
| FIT writer | `main/fit.c` | to move to littlefs | `fit`: own writer, output checked with a FIT decoder in tests |
| UTC / RTC handling | `main/utc.c` | RTC not written back to the nRF yet | `core` |
| Reference doc | `docs/HARDWARE.md` | — | — |

Not ported: `sdcard.c` / `usb_msc.c` (eMMC), `touch.c`, `ui_port.c` (LVGL), Mapsforge maps (`mapfile.c`, `mapview.c`).

## Hardware constraints
- 240x320 colour panel, **3 keys, no touch** → the whole UX is button driven.
- ESP32-S3, 2 MB quad PSRAM, app ≤ **0x420000 (4.1 MB)** per OTA slot.
- Storage: 7.8 MB littlefs `storage` partition (0x860000). **It holds the vendor's recorded rides**:
  write into a dedicated subdirectory, or back it up first. In Rust, staying compatible means
  `littlefs2` (Rust wrapper around the C littlefs, built for Xtensa). *(open: keep littlefs, or back the
  vendor rides up and use a Rust-native power-loss-safe store)*
- BLE + Wi-Fi on the ESP32; ANT+ through the nRF (its firmware is not replaced).
- 1800 mAh battery: battery life is a requirement (Wi-Fi and backlight are expensive).
- The nRF RTC is not written by c606-oss → wrong time before the first GPS fix.
- Barometric pressure + temperature come **from the nRF over UART2** (no direct sensor access from the ESP32).
  Their resolution and rate set the floor for altitude/D+ quality. See [REVERSE_NOTES.md](REVERSE_NOTES.md).
- **The nRF sensor stream (pressure, IMU) only starts after a long press of the power key.** After an ESP-only
  reset (crash, watchdog, flashing) it stays off and no known command restarts it → a ride resumed after a crash
  would have no altitude/D+. To find: a command that starts the stream; otherwise detect it, fall back to GNSS
  altitude for that segment and tell the rider.
- **IMU behind the nRF** (accelerometer, gyro, magnetometer) — unused for now, see later ideas.
- **Buzzer**: present, how it is driven is unknown (nRF command or ESP32 GPIO). See [REVERSE_NOTES.md](REVERSE_NOTES.md).

## Altitude and elevation gain (D+)
The vendor firmware gets D+ right thanks to the barometer; ours must be **at least as good**, it is part of the
core a rider checks after every ride.
- **D+ / D- from pressure only**, never from raw GNSS altitude (GNSS vertical noise of several metres inflates D+ a lot).
  Smoothing + an **hysteresis threshold** (a few metres) before counting a climb or a descent.
- **Displayed altitude = pressure-based altitude whose offset is recalibrated slowly** (GNSS altitude averaged
  over time, or the route's elevation profile / a DEM when a route is loaded). Weather drift (a front can shift
  the barometric altitude by tens of metres) affects the absolute value, much less the D+ of one ride.
- **Temperature compensation**: sudden changes (sun/shade, out of a warm car, start of a ride) distort the
  pressure reading; handle the first minutes and fast temperature swings.
- Also derived from the same filtered altitude: **gradient (%)**, VAM, D+ per lap.
- Recorded in the FIT file: per-record `altitude` (enhanced_altitude) and session/lap `total_ascent` / `total_descent`.
- Pure logic in `core` (`no_std`), fed by a `PressureSource` trait → fully testable on a PC.

## Product questions
1. **Use case**: outdoor rides, **road and MTB**, for the author or anyone with a CC600.
   Home trainer use comes later, if at all.
2. **What the vendor firmware gets wrong** and justifies the project:
   - no map around the planned route;
   - navigation through the menus is heavy, with too many useless functions.
3. **Core that must never break**: reliable FIT recording (crash, empty battery), display, sensors.
4. **What makes it different**:
   - a **corridor map around the loaded route** (see below);
   - a lean, fast 3-key UX;
   - easy ride sync (see 5).
   Later ideas: crash / long-stop detection with the IMU, fully configurable screens, BLE remotes, structured workouts, FTMS home trainer control,
   integration with home_trainer_game.
5. **Ride sync**: over **Wi-Fi when back home**, to the phone or a **configurable cloud**
   (Strava, self-hosted server…). Wi-Fi stays off during the ride.
6. **Configuration**: in a **companion app** (web app over Web Bluetooth is the current lead, to be decided).
   The device menu stays minimal: sync, sensors, and what is needed on the road. See "UI rules" below.

### UI rules (from the clickable prototype)
Prototype: https://enavarro222.github.io/opencc600/prototype/ — source in [prototype/](prototype/).
- Every screen shows what each of the 3 keys does, next to the key (top-left key = top-left label, bottom keys =
  bottom bar corners). Labels stay consistent with the front silkscreen: bottom-right = main action (start, pause,
  resume, OK), bottom-left = menu / next, top-left = back / contextual.
- No hidden action. One long press only, always labelled: "⏻ Maintenir" on the home screen to switch off (and on).
- Irreversible actions go through a confirmation screen that says what will happen; confirming uses a different
  key than the one that opened it.
- Riding: up to 4 pages, picked automatically (see "Data pages"), bottom-left cycles pages. Top-left is contextual: map zoom on the
  map page, auto-scroll on/off elsewhere.
- One banner model, floating above the page and never over the key bar: info (blue, 3 s), attention (amber, while
  true: off route, sensor lost), climb ahead, auto-pause. One at a time, by priority.
- One route on the device, sent from the app at any time (even mid-ride). Rides are uploaded as soon as possible
  (home Wi-Fi or app) and deleted only after the upload is confirmed.
- GPS: the home screen shows acquisition (state + progress bar); a ride can start before the fix (distance and
  map start at the fix). While riding, "GPS ◌" appears in the top bar only while searching or lost, with a banner.
- Pause screen: bottom-right resume, bottom-left "Finish…" (then confirmation), top "≡ Menu" (sensors, switch off).
  Switching off while paused keeps the ride; it resumes after power-on.
- A finished ride stays **resumable until it has been uploaded** ("↺ Reprendre" on the end screen, then in the
  menu): finishing only closes the FIT file, resuming reopens the same recording. Once uploaded it cannot be
  resumed (it would duplicate the activity), so "Finish" is not shown as an irreversible action.
- The 3 key labels have the same size and a blue mark (the colour of the physical keys) on the key's side:
  top edge for the top-left key (it sits on the top edge), bottom edge for the bottom keys.

### Data pages: chosen for the rider, not built by the rider
No page editor to start with. The app asks **one question** ("ride to explore" or "train"); the device detects the
rest when a ride starts (sensors seen, route loaded) and picks standard views:
- **Essential**: speed (big), time, distance, D+, HR if present (else average speed).
- **Effort**: 3 s power (big) or HR if no power meter, zone bar, then by priority avg power, HR, cadence, speed, time.
- **Map** (route loaded), **Climbs** (route with climbs), **Altitude** (no route: altitude, gradient, D+/D-, last 5 km).

| Usage | With route | Without route |
|---|---|---|
| Explore | Map · Essential · Climbs | Essential · Altitude (+ Effort if HR/power) |
| Train | Effort · Essential · Map · Climbs | Effort · Essential · Altitude |

A loaded route always brings the Map (4 pages at most). A sensor missing at start takes no slot (the next field moves up). A sensor lost mid-ride keeps its slot with
"--" plus a banner: the layout never moves during a ride. Customising (swapping a view from the catalogue) stays
possible in the app, never required.

### Features and app settings
| Feature | Default | Notes |
|---|---|---|
| Off-route alert (banner + dashed line to the route + beep) | on, beep configurable | |
| Auto-pause | on, configurable | timer stops when not moving |
| Start reminder ("you are riding, start?") | on, configurable | never starts on its own |
| Page auto-scroll | available, toggled on the device | interval set in the app |
| Night mode at sunset | on, configurable | computed from GPS position |
| Climb tracking (list, alert, live climb page) | on | climbs detected in the app when the route is prepared |
| Turn announcements | **off (later)** | cues precomputed in the app (map matching or course points from the route file) |
| Strava segments | **off (later)** | needs a Strava Live Segments licence + user subscription; starred segments otherwise |
| Laps | **off (later)** | |

### Corridor map around the route
The "no maps" limit only applies to a global map. A map restricted to the loaded route fits the CC600:
- **One route at a time**: loading a new route replaces the previous one (no history), so storage stays bounded.
- **Size**: a ±1–2 km corridor around a 100–150 km route is a few hundred km². Simplified vector data
  (roads, tracks, water, maybe coarse contours) should be in the range of a few hundred KB to 1–2 MB,
  which fits in littlefs next to the vendor FIT files. *(to measure on real routes)*
- **Prepared off the device**: the OSM extract is clipped to the corridor and pre-projected on the phone or
  a server, then pushed over Wi-Fi. The CC600 only draws polylines on its 240x320 framebuffer (`embedded-graphics`).

### MVP
- [ ] Reliable FIT recording on littlefs, in a dedicated subdirectory; survives a crash or an empty battery
      (periodic flush, file recovered at the next boot).
- [ ] Write the GPS time back to the nRF RTC (correct timestamps before the first fix).
- [ ] Barometric altitude and D+ (see above), shown on the data screens and written to the FIT file.
- [ ] 2–3 data screens driven by the 3 keys.
- [ ] Beeps (once the buzzer is understood): key feedback (can be turned off), alerts — off route, sensor lost,
      low battery, recording started/stopped. Every beep comes with a message on screen.
- [ ] Load a GPX route and display the route line (no background map yet).
- [ ] Upload FIT files over Wi-Fi to a configurable URL.

Right after the MVP: corridor background map.

### Deliberately out of scope
- global maps (Mapsforge style) and on-device routing / turn-by-turn recalculation
- …

## Technical plan
0. **Bring-up spike, first risk to clear**: an `esp-hal` image written to `ota_0` boots under the **vendor
   second-stage bootloader and partition table** (both left untouched), with the 2 MB quad PSRAM.
   Done when: backlight on + log line on the USB serial.
1. Toolchain: Rust Xtensa fork via `espup` (ESP32-S3 is Xtensa), `espflash` to build the image,
   `esptool` to write `ota_0` (see the workflow below). Pinned in `rust-toolchain.toml`.
2. Cargo workspace:

   | Crate | `no_std` | Content |
   |---|---|---|
   | `core` | yes | ride logic, screens state machine, sensors model, altitude/D+ filter, UTC/RTC, sync protocol; no hardware dependency |
   | `ui` | yes | screens drawn with `embedded-graphics` |
   | `fit` | yes | FIT writer |
   | `nrf-link` | yes | nRF frames, CRC16, handshake, keys, battery, pressure/temperature, RTC, ANT+ |
   | `gnss` | yes | NMEA parsing, `$PAIR` commands |
   | `cc600-board` | yes | esp-hal wiring: i80 panel, UARTs, backlight, flash, radio |
   | `firmware` | yes | the binary flashed to `ota_0` |
   | `sim` | no (PC) | SDL simulator, headless renderer, scenario player, fakes |

   Drivers and core only see traits (`embedded-hal`, `embedded-io-async`, `embedded-storage` and our own
   `SensorSource`, `SyncTransport`, …), so everything above `cc600-board` runs on a PC.
3. Bring-up order on the device: panel → nRF link (keys, battery, RTC) → storage → GNSS → FIT recording
   → BLE / ANT+ sensors → Wi-Fi sync.
4. Then sync / configuration / differentiating features, following the MVP.

### Planned dependencies
- Base: `esp-hal`, `esp-rtos`, `esp-alloc`, `esp-storage`, `esp-backtrace`, `embassy-executor` / `-time` / `-sync`, `defmt`
- Radio: `esp-radio`, `trouble-host` (BLE host), `embassy-net` + `reqwless` (HTTP) + `embedded-tls` or `esp-mbedtls`
- Display: `embedded-graphics`, `embedded-graphics-simulator` (PC)
- Data: `nmea`, `littlefs2` (or `sequential-storage` / `ekv`, see storage above)
- Update: an OTA path with a safe fallback (`embassy-boot` or ESP-IDF style OTA slots) — needed as soon as
  other people install the firmware
- Tests: `proptest`, `insta`, `embedded-hal-mock`, `embedded-test`

### Open technical choices
- UI: `embedded-graphics` + our own small widgets (lean) vs Slint (full framework, heavier on RAM).
- Storage: `littlefs2` on the existing partition vs a Rust-native store after backing up the vendor rides.
- Status of `esp-radio` BLE (central role, GATT client) to check early with a heart-rate strap and a power meter.

## Flash and rollback workflow (checked on 2026-10-19)
- Build: `cargo build --release`, then `espflash save-image --chip esp32s3` to get an app image for `ota_0`
  *(to validate in step 0)*. The c606-oss Docker build in REVERSE_NOTES.md stays useful to run the reference firmware.
- Flash: only `ota_0` (0x20000), then erase `otadata` (c606-oss `tools/flash_poc.py` does this and backs up first;
  we will have our own script that refuses any other offset).
- **Back to the vendor firmware**: write the original otadata back,
  `esptool -p /dev/ttyACM0 write-flash 0xd000 otadata.bin` → boots `ota_1` (V1.407).
- Keep a full 16 MB flash dump of the device before the first write.
- Recovery: holding the 3 keys hard-resets the ESP32; the ROM download mode is reachable over USB at any time.
- Tools: `esptool` (e.g. `uv tool install esptool`), OpenOCD Espressif build (`openocd-esp32` release,
  linux-amd64) with its `60-openocd.rules` udev rule, `tools/read_pins.py` for the live pinout.
- **Never** write eFuses, the bootloader (0x0) or the partition table (0x8000).
