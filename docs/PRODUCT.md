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

## Product questions (to fill in)
1. **Use case**: road, home trainer, both? For whom?
2. **What the vendor firmware gets wrong** and justifies the project:
3. **Core that must never break**: reliable FIT recording (crash, empty battery), display, sensors.
4. **What makes it different** (ideas): FTMS home trainer control, integration with home_trainer_game,
   Zwift Click / BLE remotes, fully configurable screens, structured workouts…
5. **Ride sync**: Wi-Fi (Strava / self-hosted server), BLE to an app, USB mass storage over the littlefs?
6. **Configuration**: 3-key menus, web page over Wi-Fi, app?

### MVP (to define)
- [ ] …

### Deliberately out of scope
- maps / turn-by-turn navigation (no eMMC)
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
