# opencc600 — product definition and plan

Hardware and reverse engineering background: [REVERSE_NOTES.md](REVERSE_NOTES.md).

## Decisions (2026-10-19)
- **Base**: [c606-oss](https://github.com/HighwayStar/c606-oss) is used as the **hardware layer**, not as the
  application. Its UI/app is not what we want; its drivers work on the CC600.
- **License**: GPL-3.0-or-later, so c606-oss code can be reused as is (credits kept).
- **Architecture**: move the drivers into an ESP-IDF component `cc600_bsp`, and write our own firmware on top.
- **Hardware target**: CC600 = Magene C606 board **without eMMC and without touch** (pinout checked over JTAG).

## What we take from c606-oss (BSP layer)
| Block | c606-oss files | Status on the CC600 |
|---|---|---|
| ST7789 panel, 16-bit i80 bus, LVGL 9 | `main/lcd.c`, `main/ui_port.c`, `main/backlight.c`, `main/board_c606.h`, `main/board.h` | OK |
| nRF link (UART2, CRC16 frames, handshake, keys, battery, temperature/pressure, RTC) | `main/nrf_link.c` | OK |
| ANT+ sensors via the nRF | `main/ant.c`, `main/sensor_list.c` | not tested yet |
| Airoha AG3352Q GNSS (UART0 921600, NMEA, `$PAIR`) | `main/gps.c` | OK, fix in ~6 s |
| FIT writer | `main/fit.c` | to move to littlefs |
| UTC / RTC handling | `main/utc.c` | RTC not written back to the nRF yet |
| Reference doc | `docs/HARDWARE.md` | — |

To drop or adapt for the CC600: `sdcard.c` / `usb_msc.c` (eMMC), `touch.c`, Mapsforge maps (`mapfile.c`, `mapview.c`).

## Hardware constraints
- 240x320 colour panel, **3 keys, no touch** → the whole UX is button driven.
- ESP32-S3, 2 MB quad PSRAM, app ≤ **0x420000 (4.1 MB)** per OTA slot.
- Storage: 7.8 MB littlefs `storage` partition (0x860000). **It holds the vendor's recorded rides**:
  write into a dedicated subdirectory, or back it up first.
- BLE + Wi-Fi on the ESP32; ANT+ through the nRF (its firmware is not replaced).
- 1800 mAh battery: battery life is a requirement (Wi-Fi and backlight are expensive).
- The nRF RTC is not written by c606-oss → wrong time before the first GPS fix.

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
   Later ideas: fully configurable screens, BLE remotes, structured workouts, FTMS home trainer control,
   integration with home_trainer_game.
5. **Ride sync**: over **Wi-Fi when back home**, to the phone or a **configurable cloud**
   (Strava, self-hosted server…). Wi-Fi stays off during the ride.
6. **Configuration**: 3-key menus, web page over Wi-Fi, app? *(open)*

### Corridor map around the route
The "no maps" limit only applies to a global map. A map restricted to the loaded route fits the CC600:
- **One route at a time**: loading a new route replaces the previous one (no history), so storage stays bounded.
- **Size**: a ±1–2 km corridor around a 100–150 km route is a few hundred km². Simplified vector data
  (roads, tracks, water, maybe coarse contours) should be in the range of a few hundred KB to 1–2 MB,
  which fits in littlefs next to the vendor FIT files. *(to measure on real routes)*
- **Prepared off the device**: the OSM extract is clipped to the corridor and pre-projected on the phone or
  a server, then pushed over Wi-Fi. The CC600 only draws polylines on a 240x320 LVGL canvas.

### MVP
- [ ] Reliable FIT recording on littlefs, in a dedicated subdirectory; survives a crash or an empty battery
      (periodic flush, file recovered at the next boot).
- [ ] Write the GPS time back to the nRF RTC (correct timestamps before the first fix).
- [ ] 2–3 data screens driven by the 3 keys.
- [ ] Load a GPX route and display the route line (no background map yet).
- [ ] Upload FIT files over Wi-Fi to a configurable URL.

Right after the MVP: corridor background map.

### Deliberately out of scope
- global maps (Mapsforge style) and on-device routing / turn-by-turn recalculation
- …

## Technical plan
1. ESP-IDF 5.4, target esp32s3, partition table identical to the vendor's
   (ota_0 / ota_1 0x420000, littlefs `storage` kept).
2. `cc600_bsp` component: display, nRF, GNSS, keys, battery (copied and refactored from c606-oss).
3. littlefs storage (`esp_littlefs`) mounted on `storage`, dedicated subdirectory.
4. App: LVGL UI + ride logic + FIT.
5. Then sync / configuration / differentiating features, following the MVP.

## Flash and rollback workflow (checked on 2026-10-19)
- Build: Docker `espressif/idf:v5.4.2` (command in REVERSE_NOTES.md).
- Flash: only `ota_0`, then erase `otadata` (c606-oss `tools/flash_poc.py` does this and backs up first).
- **Back to the vendor firmware**: write the original otadata back,
  `esptool -p /dev/ttyACM0 write-flash 0xd000 otadata.bin` → boots `ota_1` (V1.407).
- Keep a full 16 MB flash dump of the device before the first write.
- Recovery: holding the 3 keys hard-resets the ESP32; the ROM download mode is reachable over USB at any time.
- Tools: `esptool` (e.g. `uv tool install esptool`), OpenOCD Espressif build (`openocd-esp32` release,
  linux-amd64) with its `60-openocd.rules` udev rule, `tools/read_pins.py` for the live pinout.
- **Never** write eFuses, the bootloader (0x0) or the partition table (0x8000).
