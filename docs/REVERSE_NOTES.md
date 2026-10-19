# GEOID CC600 — reverse engineering notes

All findings below were made on one unit on 2026-10-19, over the USB-C port only (the case was never opened).

Paths starting with `dump/` refer to local files that are **not** in this repository
(flash dump of the proprietary vendor firmware).

## Hardware

- USB: `303a:1001` Espressif USB JTAG/serial debug unit → `/dev/ttyACM0`
  (native USB-Serial/JTAG of the ESP32-S3, no external UART bridge)
- SoC: **ESP32-S3 (QFN56) rev v0.2**, dual core 240 MHz, Wi-Fi + BLE 5
- PSRAM: 2 MB embedded, quad (AP_3v3)
- Flash: **16 MB**, quad, 3.3 V (manufacturer 0x85 = Puya, device 0x2018)
- Serial console: silent at boot with the vendor firmware

## Security (eFuses): everything is open

| eFuse | Value |
|---|---|
| SPI_BOOT_CRYPT_CNT (flash encryption) | Disable |
| SECURE_BOOT_EN | False |
| KEY_PURPOSE_0..5 | USER (no key) |
| DIS_DOWNLOAD_MODE / DIS_USB_SERIAL_JTAG_DOWNLOAD_MODE | False |
| DIS_USB_JTAG / DIS_PAD_JTAG / SOFT_DIS_JTAG | False / False / 0 |

→ the flash can be freely read and written with esptool, and the chip debugged over the same USB cable (OpenOCD).
esptool reaches the ROM loader while the device is running, no button combination needed.

## Flash layout

Full 16 MB dump: `esptool -p /dev/ttyACM0 -b 921600 read-flash 0 ALL flash_full_16MB.bin` (~100 s, read went through
with the stub in one pass). Keep it: it is the only way back to a fully stock device.

### Partition table (0x8000)

| Name | Type | Offset | Size | Notes |
|---|---|---|---|---|
| nvs | data/nvs | 0x009000 | 16K | settings, probably BLE pairings |
| otadata | data/ota | 0x00d000 | 8K | seq 1 / seq 2 → **ota_1 active** |
| phy_init | data/phy | 0x00f000 | 4K | |
| coredump | data/coredump | 0x010000 | 64K | holds a crash ELF |
| ota_0 | app | 0x020000 | 4224K | `Venus_V1.313-dirty`, Aug 21 2024, IDF `e3fda883-dirty` (old, inactive) |
| ota_1 | app | 0x440000 | 4224K | `506_SE_V1.407-dirty`, Mar 5 2026, IDF `17977806-dirty` (active) |
| storage | data/0x83 (littlefs) | 0x860000 | 7800K | `/littlefs/FITS/FIT/*.fit` = recorded rides, `ModuleDataTest`, … |

Both apps are standard ESP-IDF images (valid checksum and SHA256, project name "Project", secure version 0).

## The CC600 is a rebadged Magene C606

- Geoid is Magene's sister brand (Qingdao). The CC600 firmware holds the strings `CC600` and `MageneC606` side by side.
- FCC: the CC600 is most likely [2A2L5-345](https://fccid.io/2A2L5-345) (Geoid, 2024-09-25), a "change in ID" of
  [2ALZG-309](https://fccid.io/2ALZG-309) (Magene, 2024-08-26), whose filing has
  [internal photos of the PCB](https://fccid.io/2ALZG-309/Internal-Photos/Internal-Photos-7601615).
- The CC700 Pro is [2A2L5-390](https://fccid.io/2A2L5-390), close to the Magene C606 Pro.
- Same memory configuration as the C606: 16 MB flash + 2 MB quad PSRAM.

### Existing project: c606-oss

[HighwayStar/c606-oss](https://github.com/HighwayStar/c606-oss) (GPL-3.0; fork [an0nfunc/c606-oss](https://github.com/an0nfunc/c606-oss)
lags behind): open firmware (ESP-IDF + LVGL 9) for the Magene C606 / C606 Pro / C706 and the Geoid CC700 Pro.
Its `docs/HARDWARE.md` is the reference for the board: C606 pinout, ST7789 init sequence, nRF protocol, GNSS commands.

C606 hardware according to c606-oss:
- 240x320 **ST7789 panel on a 16-bit i80 bus**, FT6336 / CST328 touch (I2C)
- **nRF co-processor**: buttons, power, charging, RTC, ANT+/sensor radios. UART link with framed messages and CRC16,
  and a vendor power-on handshake. c606-oss only replaces the ESP32 app, never the nRF firmware.
- Airoha AG3352Q GNSS on UART0 (NMEA + `$PAIR` commands)
- **Barometric pressure sensor** (listed in the CC600 specs; the C606 and CC700 have one too). It is **read by the
  nRF**, not by the ESP32: pressure and temperature arrive as nRF frames on UART2. This is why the vendor
  firmware's elevation gain (D+) is accurate: it is computed from pressure, not from GNSS altitude.
  According to c606-oss `docs/HARDWARE.md`: nRF sensor stream `cmd 0x10`, payload `F1 03`, ~5 Hz,
  temperature int16 @2 in 0.01 °C, pressure u32 @4 in 0.01 Pa (so ~0.1 m resolution). The vendor uses
  `44330 * (1 - (p/1013.25)^0.19)`. The stream only starts after a **long press of the power key**: after an
  ESP-only reset it stays off, and no known ESP32 command starts it. The same stream carries accelerometer (`F1 01`),
  gyro (`F1 02`) and magnetometer calibration (`F1 00`) → there is an IMU behind the nRF.
  *(To check on the CC600: same frames, actual rate.)*
- **Buzzer**: the CC600 beeps, but nothing is known yet about how. c606-oss documents no buzzer and no beep command.
  The live pinout (JTAG snapshot taken while not beeping) shows no PWM on the ESP32 except the backlight (GPIO45),
  so it is either driven by the **nRF** (like keys and power; candidate: the undocumented ESP32→nRF
  `E2 01 XX` "LED/misc" command, XX < 0x1c), or by an ESP32 GPIO configured only while beeping.
  *(To find: FCC internal photos of 2ALZG-309 (where the buzzer sits), `beep`/`buzzer` strings in the vendor app,
  a JTAG pin snapshot / UART2 TX breakpoint during a vendor beep.)*
- 4 GB eMMC mounted as FAT on `/sdcard`

Recovery: holding all three buttons hard-resets the ESP32.

### CC600 vs C606: no eMMC

- C606 partitions: ota_0 / ota_1 of 0x73A000 each, **no storage partition** (all data on the eMMC).
- CC600 partitions: ota_0 / ota_1 of 0x420000 + **7.8 MB littlefs on the internal flash**.
- CC600 firmware: no `/sdcard` string, 89 `/littlefs` strings (FITS, TMP, ModuleDataTest…), brand strings
  `BRAND_OVERSEA_GEOID` / `BRAND_*_MAGENE`. Consistent with the CC600 having no maps.

## Live pinout read over JTAG

`tools/read_pins.py` attaches OpenOCD (Espressif build, `board/esp32s3-builtin.cfg`) to the running vendor firmware
without a reset, halts the CPUs for about one second, dumps the GPIO matrix / IO_MUX / GPIO / UART / LCD_CAM
registers and resumes. Read-only. Raw output: [pins_live.txt](pins_live.txt).
Needs OpenOCD's `60-openocd.rules` udev rule for non-root USB access.

| Function | CC600 (live) | c606-oss `BOARD=c606` |
|---|---|---|
| LCD i80 **16-bit** D0..D15 | 4,38,5,37,6,36,7,35,8,34,9,33,10,47,11,48 | same |
| LCD DC / WR / CS / RD | 40 / 3 / 2 / 39 (driven high) | same |
| Backlight | 45 (LEDC) | 45 |
| nRF UART2 TX / RX | 42 / 41, 115200 baud | 42 / 41, 115200 |
| GNSS UART0 TX / RX | 1 / 0, ~921600 baud | 1 / 0, 921600 |
| Touch I2C SDA 21 / SCL 12 | **not configured** → no touch | FT6336 / CST328 |
| SDMMC 13-18 | **not configured** → no eMMC | 4 GB eMMC |
| GPIO43/44 | default U0TXD/U0RXD (not driven high) | — |

**Conclusion: the CC600 is a C606 board without eMMC and without touch.** Display, nRF and GNSS are identical.

## First run of c606-oss `BOARD=c606`

- Build with the ESP-IDF container (the repo's script uses podman; with Docker):
  `docker run --rm -u $(id -u):$(id -g) -e HOME=/tmp -v "$PWD:/project" -w /project -e IDF_TARGET=esp32s3 espressif/idf:v5.4.2 idf.py -B build -DBOARD=c606 build`
  → `build/c606_oss.bin` = 0xF5E20 bytes (~1 MB), fits in the 0x420000 ota_0 slot.
- Flash with `tools/flash_poc.py -p /dev/ttyACM0 --board c606` (from c606-oss): backs up ota_0 + otadata,
  writes ota_0, erases otadata. It reads the partition table from the device, so the CC600 layout works as is.
- Gotcha: `--dry-run` leaves the ESP32 in the ROM loader, so the screen goes dark;
  `esptool -p /dev/ttyACM0 --after hard-reset read-mac` boots it again.
- At boot: display / LVGL OK, nRF link and handshake OK, GNSS `AG3352Q_V3.1.0` detected at 921600 baud,
  fix in ~6 s indoors, key 2 starts a ride. The eMMC mount fails as expected (no formatting attempted), so the ride
  runs **without recording**.
- Back to the vendor firmware: `esptool -p /dev/ttyACM0 write-flash 0xd000 otadata.bin` with the original otadata
  → the bootloader loads ota_1 (V1.407) again. Done and checked on 2026-10-19.
- Verdict: the drivers work on the CC600, but the c606-oss app is not what we want → c606-oss becomes our
  hardware layer, with our own firmware on top. See [PRODUCT.md](PRODUCT.md).

## First run of our Rust spike (step 0)

- The vendor second-stage bootloader logs on the USB-Serial/JTAG port (partition table, segments), then loads
  our esp-hal image from ota_0. Quad PSRAM 2 MB OK.
- **The panel keeps its frame memory across an ESP32 reset**: its reset line is on the nRF side
  (c606-oss `LCD_PIN_RST -1`), so an esptool reset does not clear it. With only the backlight turned on, the
  spike showed the vendor's last frame ("Batterie actuelle: 100%", its USB charging screen). → the panel init must
  clear the GRAM (SWRESET + fill) before the backlight goes on, or stale frames from another firmware show up.

- **Panel with esp-hal** (`firmware/src/panel.rs`): LCD_CAM i8080 16-bit at 15 MHz, the vendor ST7789 init replayed
  as is, MADCTL `0x00` (RGB order and orientation correct), 8-bit commands/parameters (padded on the 16-bit bus),
  pixels as little-endian RGB565 words with the default (native) byte order. Test pattern checked on the
  device: orientation, colour order, smooth 32-step ramps on R/G/B (all 16 data lines OK), no offset.

- **Display timings** (boot bench in `firmware/src/bin/main.rs`, 240 MHz, 20-run averages):

  | | PSRAM 40 MHz (esp-hal default), per-pixel copy | PSRAM 80 MHz (vendor setting), memcpy |
  |---|---|---|
  | full framebuffer flush | 16.3 ms (copy 11.0 + transfer 5.3) | **11.1 ms** (copy 5.8 + transfer 5.3) |
  | solid fill (no PSRAM read) | 7.9 ms (fill 2.6) | 6.3 ms (fill 1.0) |
  | framebuffer clear in PSRAM | 19.5 ms | 10.5 ms |
  | test pattern drawn in PSRAM | 31 ms | 18 ms |

  The transfer is at the bus limit (76 800 px at 15 MHz = 5.12 ms; 15 MHz is the ST7789 write-cycle limit and
  the vendor's setting). What is left is the CPU copy PSRAM → DMA buffer: two ping-pong DMA buffers would bring a
  full frame down to ~6 ms, partial (dirty-area) updates further. Not needed for a 1 Hz data screen; to do with
  the move to async transfers, so the CPU is not blocked during a flush.

## nRF link on the CC600 (`crates/nrf-link`, 2026-10-19)

Same framing, CRC and messages as the C606 documented by c606-oss, with these differences and measurements:

- **nRF firmware 1.2.16** (`cmd 01` version frame; the C606 in c606-oss reports 0.2.19).
- Every `SendPowerOnCmd` (sent every 5 s, like c606-oss) is answered by an `Ack`, the power-on reason and the version.
  Power-on reason seen: 4 (manual) at the first boot, then 5 and 6 after esptool resets with USB plugged in.
- **Power off checked**: `SendPowerOffCmd` (`E2 02 00 00 00 00 00 00`) cuts the ESP32 at once, **even with USB
  plugged in** (the USB-Serial/JTAG port disappears). A long press on the power key then boots our ota_0 image
  again from cold, power-on reason 4 (manual).
- Keys: indexes 0..2, `1` click on release, `4` long press repeated while held, `5` release after a long press
  (all three keys checked). A long press on key 0 also makes the nRF send its power-on reason and version again.
- Rates (host timestamps, 30 s): RTC and barometer **5 Hz**; battery, status (`F0`, 0 with USB plugged in at
  100 %) and the stream subtypes `04` / `0A` **3.3 Hz** (300 ms).
- The RTC keeps UTC set by the vendor firmware (checked against the host clock).
- **Barometer**: as on the C606, the stream starts only after a long press of key 0. Resolution 0.01 Pa,
  **noise 0.8 Pa standard deviation at rest (≈ 7 cm of altitude)**, range 2.8 Pa over 30 s.
  The temperature reads ~30 °C in a 20–22 °C room, rising slowly: the sensor is inside the case, heated by the
  ESP32 (240 MHz, never sleeping), the backlight and the charger. Fine for the sensor's own compensation, not an
  air temperature. *(To compare with the vendor firmware in the same conditions.)*
- **Ambient light sensor = stream `F1 04`** (checked by covering / lighting the device): u16 @6 = raw level
  (~1000 indoors at night), u32 @2 = constant 600 (unknown). Vendor factory test strings `Environment Light:`,
  `LightLux:`, setting `Auto(Light)` (auto backlight).
- **No accelerometer / gyro frames** (`F1 01` / `F1 02`) on the CC600. Two subtypes not seen on the C606, meaning
  unknown: `F1 0A` = u16 ~3000..3100, tracks the barometer temperature 0.01 °C a little above it (second
  temperature? to check by warming the case).
- **Buzzer = nRF command `E2 01 XX`** (type 2, cmd 0x10, `E2 01 XX 00 00 00 00 00`), every value acknowledged.
  The vendor firmware has a `BUZZER FUNC TEST` / `PRESS B KEY TEST BUZZER` factory test and `Key Tones` /
  `Prompt Tones` settings. Scan of 00..1B recorded with the PC microphone (`tools/sound_probe.py`, piezo
  fundamentals 1.5–7 kHz).
  Heard: 00 and 01 the same short beep, 02 "ti-dou-di", 17 "tou-di-dou"; 13, 18, 19 silent by ear.

  | `E2 01 XX` | notes (name ms; `→` legato, `· N ms ·` silence) | total |
  |---|---|---|
  | 13 | silent | |
  | 00 | F#7 (3.00 kHz) 80 | 80 ms |
  | 01 | F#7 (3.00 kHz) 160 | 160 ms |
  | 02 | D#8 (4.90 kHz) 70 → C8 (4.25 kHz) 80 · 30 ms · D#8 (5.00 kHz) 160 | 350 ms |
  | 03 | D#8 (4.90 kHz) 80 → F8 (5.60 kHz) 70 · 40 ms · C8 (4.25 kHz) 70 · 30 ms · F8 (5.50 kHz) 160 | 460 ms |
  | 04 | F#7 (3.00 kHz) 60 → G#7 (3.40 kHz) 90 · 30 ms · F#7 (3.00 kHz) 60 · 40 ms · F#7 (3.00 kHz) 70 | 360 ms |
  | 05 | D#8 (4.90 kHz) 70 → D#8 (5.00 kHz) 80 · 120 ms · D#8 (4.90 kHz) 80 · 90 ms · D#8 (5.00 kHz) 80 | 530 ms |
  | 06 | F8 (5.60 kHz) 120 → D#8 (5.00 kHz) 80 · 30 ms · D8 (4.80 kHz) 300 | 540 ms |
  | 07 | D8 (4.80 kHz) 160 → F#8 (6.00 kHz) 80 · 20 ms · F8 (5.60 kHz) 310 | 570 ms |
  | 08 | F#7 (3.00 kHz) 70 → G#7 (3.40 kHz) 80 · 30 ms · F#7 (3.00 kHz) 70 · 30 ms · F#7 (3.00 kHz) 70 | 360 ms |
  | 09 | G#7 (3.40 kHz) 80 → F#7 (3.00 kHz) 70 · 30 ms · G#7 (3.40 kHz) 80 · 30 ms · F#7 (3.00 kHz) 70 | 370 ms |
  | 0A | B7 (4.00 kHz) 110 | 110 ms |
  | 0B | B7 (4.00 kHz) 110 · 100 ms · B7 (4.00 kHz) 50 | 260 ms |
  | 0C | F#8 (6.00 kHz) 250 · 210 ms · B7 (4.00 kHz) 50 | 510 ms |
  | 0D | B7 (4.00 kHz) 410 | 410 ms |
  | 0E | B7 (4.00 kHz) 410 · 50 ms · B7 (4.00 kHz) 410 | 870 ms |
  | 0F | B7 (4.00 kHz) 260 · 150 ms · B7 (4.00 kHz) 250 · 150 ms · B7 (4.00 kHz) 260 | 1070 ms |
  | 10 | F#8 (6.00 kHz) 240 · 210 ms · B7 (4.00 kHz) 60 | 510 ms |
  | 11 | B7 (4.00 kHz) 150 · 160 ms · F#8 (6.00 kHz) 250 | 560 ms |
  | 12 | B7 (4.00 kHz) 2870 | 2870 ms (still sounding at the next command) |
  | 13 | B7 (4.00 kHz) 70 | 70 ms |
  | 14 | G#7 (3.40 kHz) 80 · 30 ms · F#7 (3.00 kHz) 70 · 40 ms · F#7 (3.00 kHz) 70 · 30 ms · F#7 (3.00 kHz) 70 · 30 ms · G#7 (3.40 kHz) 80 · 30 ms · F#7 (3.00 kHz) 70 · 40 ms · F#7 (3.00 kHz) 70 | 710 ms |
  | 15 | D#7 (2.50 kHz) 140 | 140 ms |
  | 16 | F#7 (3.00 kHz) 310 | 310 ms |
  | 17 | F8 (5.60 kHz) 150 → D#8 (5.00 kHz) 80 · 30 ms · D8 (4.80 kHz) 310 | 580 ms |
  | 18 | silent | |
  | 19 | silent | |
  | 1A | C#8 (4.50 kHz) 80 → C#8 (4.55 kHz) 70 · 30 ms · E8 (5.40 kHz) 90 · 40 ms · B7 (3.90 kHz) 70 | 390 ms |
  | 1B | G6 (1.55 kHz) 70 | 70 ms |

  Spectrograms of all of them: [buzzer_sounds.png](buzzer_sounds.png) (the reference: the table can miss a
  quieter last beep, e.g. 0B is 3 beeps). Same sound twice: 06 = 17, 04 = 08, 0C = 10; 1B is noise only.

  12 keeps sounding (4 kHz) until the next command: probably a continuous alarm that needs a stop command
  (13, which gave a 90 ms tick right after it, or 18 / 19?). *(To check: what stops 12; which sound the vendor
  uses for keys, ride start / stop / lap, alerts.)*
- **UART FIFO**: 128 bytes = ~11 ms at 115200 baud. Reading it from the thread-mode executor lost frames
  (`FifoOverflowed`) during a redraw + flush (~25 ms); the nRF task now runs on an interrupt executor. The GNSS at
  921600 baud fills the FIFO in 1.4 ms: it will need UART DMA (UHCI) or the same high-priority reader.

## Vendor storage partition (littlefs)

- littlefs **v2.1** on-disk, block size 4096, 1950 blocks; 1324 used (68 %) on 2026-10-19.
- Tree: `/FITS/FIT/<unix start>.fit` (rides), `/FITS/LAP/<same>.lap` (laps, a few u32), `/FITS/Recorder.db`
  (ride index, binary, format unknown), `/LOG/<date>.log` (vendor logs, up to ~450 KB a day), `/CONFIG/`
  (`ProductInfo.config`, `sensor.bin`, `wifi.bin`, `WifiList.config`, `user_info.bin`, `riding_mode*.bin`,
  `<t>_agnss.bin` 157 KB of A-GNSS data, `avatar_file.bmp`…), `/ABNORMAL/CoredumpInfo.bin`, `/APP/`, `/NOTIFY/`.
- **`littlefs2` (Rust) cannot mount it**: the crate pins the on-disk version to 2.0 (`DISK_VERSION`, littlefs
  multiversion), the vendor formatted 2.1 → `lfs_mount` fails with `-22` "Invalid version v2.1 != v2.0", on the
  device and on a PC alike. Irrelevant once we format the partition ourselves (option b); the same error on both
  sides shows the Xtensa build of littlefs reads the flash correctly.
- **Ride files are compressed FIT**: `".ZIP"` + the 14-byte FIT header + `{u16 LE length, zlib stream}*` + the
  2-byte FIT CRC. Inflated, header + records + CRC is a valid FIT file (all 24 rides decode, CRC OK; ~30 % smaller
  stored). `tools/vendor_fit.py` does the conversion.

## Safety rules

- **Never** write eFuses (irreversible).
- Do not overwrite the bootloader (0x0) or the partition table (0x8000).
- Only write ota_0 (+ otadata), keep the vendor firmware in ota_1.
- Keep the full flash dump before any write.
