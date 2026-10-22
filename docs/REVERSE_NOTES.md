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
  gyro (`F1 02`) and magnetometer calibration (`F1 00`) → an IMU behind the C606 nRF (the CC600 sends none of these frames, see the nRF link section).
  *(To check on the CC600: same frames, actual rate.)*
- **Buzzer**: driven by the nRF, command `E2 01 XX` (c606-oss documents none). Sounds in the nRF link section
  below.
- 4 GB eMMC mounted as FAT on `/sdcard`

Recovery: holding all three buttons hard-resets the ESP32.

### CC600 vs C606: no eMMC

- C606 partitions: ota_0 / ota_1 of 0x73A000 each, **no storage partition** (all data on the eMMC).
- CC600 partitions: ota_0 / ota_1 of 0x420000 + **7.8 MB littlefs on the internal flash**.
- CC600 firmware: no `/sdcard` string, 89 `/littlefs` strings (FITS, TMP, ModuleDataTest…), brand strings
  `BRAND_OVERSEA_GEOID` / `BRAND_*_MAGENE`. Consistent with the CC600 having no maps.

## Live pinout read over JTAG

`tools/reverse/read_pins.py` attaches OpenOCD (Espressif build, `board/esp32s3-builtin.cfg`) to the running vendor firmware
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

- **Display timings** (boot bench in `firmware/src/bin/opencc600/screen.rs`, 240 MHz, 20-run averages):

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
- Every `SendPowerOnCmd` is answered by an `Ack`, the power-on reason and the version.
  Power-on reason seen: 4 (manual) at the first boot, then 5 and 6 after esptool resets with USB plugged in.
- **Sent only until the nRF answers, as the vendor** (2026-10-20): repeated every 5 s (as c606-oss, harmless on its
  C606), it killed every ANT+ channel and search on the CC600 within 1–4 s (state 4, one or a few pages, searches
  hearing nothing). Once it stopped, a heart rate strap sent its pages for minutes.
- **RTC write found and checked**: the ESP32 sends the nRF's own RTC frame back as a type 2 (set) frame,
  `A5 0C 6F F1 02 00 | 53 FF ss mm hh dd MM yy | CRC`, day byte = day | weekday << 5 (0 = Sunday, as in the frames the
  nRF sends: `0x91` = Thursday 17), **but month 1..12** where the nRF reports 0..11 (writing month - 1 moved the RTC
  back one month at each write; repaired). Applied at once, reported back within a second.
- **The nRF can switch the GNSS off**: after a power cycle the receiver stayed silent until a long press on key 0
  (the nRF's power-on gesture), then cold-started. Our firmware must send `gps_power(1)` at boot.
- **Power off checked**: `SendPowerOffCmd` (`E2 02 00 00 00 00 00 00`) cuts the ESP32 at once, **even with USB
  plugged in** (the USB-Serial/JTAG port disappears). A long press on the power key then boots our ota_0 image
  again from cold, power-on reason 4 (manual).
- Keys: **0 top left, 1 bottom left, 2 bottom right** (screen facing you). Indexes 0..2, `1` click on release, `4` long press repeated while held, `5` release after a long press
  (all three keys checked). A long press on key 0 also makes the nRF send its power-on reason and version again.
- Rates (host timestamps, 30 s): RTC and barometer **5 Hz**; battery, status (`F0`, 0 with USB plugged in,
  charging or at 100 %, 1 on battery: read in the ride logs) and the stream subtypes `04` / `0A` **3.3 Hz** (300 ms).
- The RTC keeps UTC set by the vendor firmware (checked against the host clock).
- **Barometer**: as on the C606, the stream starts only after a long press of key 0. Resolution 0.01 Pa,
  **noise 0.8 Pa standard deviation at rest (≈ 7 cm of altitude)**, range 2.8 Pa over 30 s.
  The temperature reads ~30 °C in a 20–22 °C room, rising slowly: the sensor is inside the case, heated by the
  ESP32 (240 MHz, never sleeping), the backlight and the charger. Fine for the sensor's own compensation, not an
  air temperature. *(To compare with the vendor firmware in the same conditions.)*
- **Ambient light sensor = stream `F1 04`** (checked by covering / lighting the device): u16 @6 = raw level
  (~1000 indoors at night), u32 @2 = measurement tag: the nRF cycles 600 (the ambient light: ~2500 by day indoors), then 2000 and 3000 (level < 10 indoors, meaning unknown; seen 2026-10-19, earlier captures only had 600). Checked by hand: tag 600 ~2517 indoors by day, 411 covered, 2596 under a phone torch (saturates near 2600?); tags 2000 / 3000 stay at 0–6 in all three cases (not visible light: infrared, or something else). Vendor factory test strings `Environment Light:`,
  `LightLux:`, setting `Auto(Light)` (auto backlight).
- **No accelerometer / gyro frames** (`F1 01` / `F1 02`) on the CC600. Two subtypes not seen on the C606, meaning
  unknown: `F1 0A` = u16 ~3000..3100, tracks the barometer temperature 0.01 °C a little above it (second
  temperature? to check by warming the case).
- **Buzzer = nRF command `E2 01 XX`** (type 2, cmd 0x10, `E2 01 XX 00 00 00 00 00`), every value acknowledged.
  The vendor firmware has a `BUZZER FUNC TEST` / `PRESS B KEY TEST BUZZER` factory test and `Key Tones` /
  `Prompt Tones` settings. Scan of 00..1B recorded with the PC microphone (`tools/sound_probe.py`, removed since
  with the firmware's scan mode: `git show 8b69898:tools/sound_probe.py`; piezo fundamentals 1.5–7 kHz).
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
- **Flash writes stall the UART readers**: while littlefs programs or erases a sector, the flash cache is off and
  code running from flash waits, the interrupt executor included. During the storage HIL test (back-to-back
  appends) the nRF FIFO overflowed 6 times in ~3 min. Rides write far less often, but the GNSS at 921600 baud
  will lose bytes on every sector erase: its reader needs UART DMA (UHCI) or an IRAM interrupt handler that keeps
  draining the FIFO.

## The nRF's sensor stream and the vendor's power-on (2026-10-20)

- **The stream (`F1` barometer / light / IMU and the `00 53` RTC frames, 5 Hz) only starts on the nRF's own
  power-on gesture**, a long press of key 0, whatever the ESP32 is doing then (checked: a long press on a screen
  where our UI ignores it starts it, the ESP32 keeps running). It survives ESP32-only resets (esptool, RTS, a
  flash, `software_reset`) and stops on a real power cut of the ESP32: `SendPowerOffCmd` then the charger starting
  it again, unplugging, an empty battery. Without it the nRF still sends the battery, the `F0` status, the keys,
  its version and the replies: the device runs without clock (until a GNSS fix) and without barometer.
- **The vendor never starts it either**: its 17 functions that send to the nRF (callers of the frame builder
  `FUN_4203b1e4`, Ghidra on ota_1) send only `24 01 FF FF 00 FF FF FF` (type 2, cmd 1: `SendQuerySlaveModeCmd`) and
  `E2` commands:

  | `E2 grp id 00 00 val 00 00` | vendor |
  |---|---|
  | `E2 02 00 .. 01` / `.. 00` | `SendPowerOnCmd` / `SendPowerOffCmd` (`SendCheckPowerOnReasonCmd`: the same as type 1) |
  | `E2 02 01 .. 02` | `SendPowerRstCmd`, 2 s after a firmware upgrade: the nRF power-cycles the ESP32 (USB port gone, back at once); **the stream does not start** (tried) |
  | `E2 02 07 .. 0/1/2` | GPS power |
  | `E2 02 08 .. 00` | `SendForbidChargeCmd` (c606-oss read it as "factory init") |
  | `E2 01 XX` | the buzzer sounds (see above); the factory test toggles 12 / 13 (long beep on, short beep) |

  `E2 02 03` (LCD) and `E2 02 09` (display module) are sent from other paths (c606-oss); ids 2, 4, 5, 6 never seen.
- **How the vendor lives with it**: `ArmSendPowerOnSysEvent` maps the power-on reason 4 (manual) to the power-on,
  5 / 6 (the charger) to `BOOTUP_POWER_OFF_CHARGE`: the ESP32 runs a charging page only (`PagePowerOffChargeInit`)
  and the user's long press is the real power-on, which starts the stream. Our firmware does the same: the charging
  screen whenever the stream is missing on USB power (`Screen::Charging`), the long press of the home screen not a
  power off without the stream.
- `dump/vendor_files/APP/…/N51_V1.602_A.bin` (70 KB, from the vendor's `/littlefs/APP`) is encrypted or scrambled
  (no strings, no plain Cortex-M vector table): maybe a slave MCU image, not read further.

## GNSS on the CC600 (`crates/gnss`, `firmware/src/gnss_uart.rs`, 2026-10-19)

- Airoha AG3352Q on UART0 at 921600 baud, NMEA on by default (as on the C606), powered at boot without any
  command. Fix indoors in seconds: 16–18 satellites used out of ~37 in view (GPS, GLONASS, Galileo, BeiDou),
  HDOP ~0.8.
- **Output: one burst per second, ~2.6 KB, ~23 sentences**, in this order: `GGA`, `GLL`, `GSA` ×4 (one per
  constellation, NMEA 4.1 system id), `GSV` (GP/GL/GA/GB), `RMC`, `VTG`, `ZDA`, Airoha `GNACCURACY`, `GNEPE`,
  `PLSV`, `PAIRDGP`, and `$PAIRSAT` satellite reports (~300 characters, longer than NMEA allows; dropped).
  **GGA comes before RMC**: an epoch is complete when RMC arrives after the GGA of the same second.
- **Reception through UHCI DMA** into a 16 KB ring in internal RAM; `uart_idle_eof_en` (set by the UHCI *TX*
  config, shared register) closes a descriptor when the line goes idle, so each burst is handed over at once.
  The transfer's `is_done()` then rises at every idle EOF: silence is the only sign of a stopped DMA.
  **Checked during continuous littlefs writes** (storage HIL build, 379 appends with sector erases): 921 sentences,
  0 bytes lost, while the plain UART FIFO readers lose data during flash operations.
- **Time without a fix is not trustworthy**: without a fix (indoors, 0 satellites used), RMC (status `V`) and
  `ZDA` still carry a time and date, but this capture had them **~22 s behind** the nRF RTC and the host clock,
  which agreed with each other to ~2 s. The RTC is therefore only set from a valid fix (RMC `A`). Also seen
  at boot: `PAIR010` aiding requests (GPS week 2441, TOW) and `PAIR001` acks for commands 002, 021 and 382,
  which the ESP32 did not send.
- **Output setup** (`gnss::OUTPUT_SETUP`, checked 2026-10-19): `$PAIR062,<type>,<rate>` is acknowledged
  (`PAIR001,062,0`) for types 0–10 (0 GGA, 1 GLL, 2 GSA, 3 GSV, 4 RMC, 5 VTG, 6 ZDA, 7–10 off by default), types ≥ 11
  are refused (result 4); `$PAIR063,<type>` reads a rate back, `-1` lists them all. We keep GGA and RMC every
  fix and GSV every 5 s: **~19 → ~7 sentences/s, ~2.0 → ~1.6 KB/s**. The setup is kept while the receiver stays
  powered (it survives an ESP32 reset) and lost when it restarts; the firmware sends it again when a turned-off
  sentence shows up.
- **Left over, not controlled by `PAIR062`**: `GNACCURACY`, `GNEPE`, `PLSV`, `PAIRDGP` (1/s each) and `$PAIRSAT`
  (4/s, ~300 chars, ~1.2 KB/s: most of the remaining traffic). The vendor's `$PAIR866,0,0,0` is accepted with no
  visible effect, `$PAIR867,0/1` is refused (4). Probably custom outputs of the Magene receiver firmware; to find.
- **`gps_power(1)` (`E2 02 07 01`) restarts a running receiver**: it reboots (`PAIR011,001`, then acks for
  `PAIR002`, `021`, `382` sent by its own start-up) and loses its output setup. The firmware sends it only after
  3 s of silence. *The silent-receiver path is not tested on the device yet.*
- **Start messages** (Airoha PAIR, decoded in `crates/gnss`): `PAIR011,001` = the receiver started;
  `PAIR010,<type>,<system>,<GPS week>,<TOW>` = aiding request, type 0 EPO per constellation (0 GPS, 1 GLONASS,
  2 Galileo, 3 BeiDou), 1 time (`PAIR010,1,-1`), 2 position (`PAIR010,2,-1`). Seen after a receiver start: it
  asks for EPO, time and position, which we do not provide yet. Restarts: `$PAIR004` hot, `005` warm, `006` cold,
  `007` full cold; `$PAIR590,YYYY,MM,DD,hh,mm,ss` = UTC time aiding (from the vendor strings, *ack not checked
  yet*). The firmware counts these (trip log columns `gnss_*`) and logs `gnss: first fix N s after <start>`;
  `tools/ttff.py` measures the time to first fix per kind of start.
- **Long time to first fix after a power cycle (ride of 2026-10-19)**: 0 satellites for 37 min with the device in
  a pocket outdoors, then 29 at once, still in the pocket. Likely a cold start (the nRF cut the receiver when
  unplugged) with the body attenuating the signal (acquisition needs a stronger signal than tracking). The vendor
  parks the receiver (`$PAIR650`), gives A-GNSS data and probably the time (`$PAIR590`). To measure with
  `tools/ttff.py`.
- **Vendor GNSS commands** (ota_1 strings; reply parser `FUN_420ec2b0`): restarts `$PAIR004`..`007`, `$PAIR020`
  (version, 6 chars kept), `$PAIR023`, `$PAIR382,0/1`, `$PAIR866,0,0,0`, `$PAIR867`, time `$PAIR590,`, and EPO:
  `$PAIR490,0` is sent with the log line `CloseEPOCSend` (end of an EPO upload), `$PAIR491` reads the EPO status
  (vendor logs: `PAIR491,1,0` and once `PAIR491,1,3`), `$PAIR470` lines are its data or status. The parser waits
  for `PAIR001,650` (parking ack). **No position aiding command** in the strings (no `PAIR600`). The vendor logs
  `GpsPthreadGetAgnssFileInfor:AgnssTimeOut!` at its last boots: its A-GNSS file had expired, so the vendor started
  without EPO too; what it had more than us is the parking and the time.
- Fixtures: `crates/gnss/tests/fixtures/` holds an anonymised 10 s capture (fake position, randomised satellite
  geometry). Raw captures contain the real position and stay in `dump/`.

## USB-Serial/JTAG link (2026-10-19)

- **Lines are lost in both directions when the log is busy**: ~5–7 % of the host → device command lines (82 sent,
  76 counted by the firmware) and some log lines, around the nRF task's bursts of log lines (every 5 s: RTC and
  the replies to the power-on command, written from the interrupt executor). Reading the RX FIFO inside a
  critical section did not help. Fewer periodic log lines (moved to debug) made it rare; the test hook numbers and
  acknowledges commands (see DEV_PROCESS.md). *Root cause not found.*
- The last log line can stay in the TX FIFO until the next one is written (it shows up late on the host).

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
  stored). `tools/reverse/vendor_fit.py` does the conversion.

## Vendor altitude and D+ (2026-10-19)

From the vendor app in ota_1 (decompiled with Ghidra 12.1, see *Tools* below) and its 24 ride files (`dump/fits`).
Our design: [ALTITUDE.md](ALTITUDE.md). Addresses are in the ota_1 image as mapped (flash code 0x42.., rodata
0x3C1E.., the globals in PSRAM 0x3C3A..).

**Pressure → altitude** (`FUN_4203bd74`, the nRF frame dispatcher, branch `F1 03`): temperature = int16 / 100 °C,
pressure = u32 / 100 Pa, then in double:

    h = 44330 × (1 − (p_hPa / 1013.25)^0.190295)     (standard atmosphere, exponent 1/5.255), stored × 10 (dm)

|h| ≤ 1e-6 is stored as 0. Same formula as ours to < 0.2 m per 1000 m.

**Calibration: once, on the first fix** (`FUN_4214217c`, the big activity task, around the `*baseAltitude`
strings). While no base is set: if the phone app gave an altitude, `base = app altitude`
(`APPbaseAltitude`), else on the first valid GNSS fix `base = GGA altitude` (`GPSbaseAltitude`), a single
sample. The barometric altitude at that moment is kept, then

    altitude = (baro altitude now − baro altitude at calibration) + base     (base clamped to −2000..100000 m)

No continuous GNSS correction: the offset stays for the whole session (the −12 m step seen in one ride is not
from here). In mode 5 of a session field (`+0x16c`, an indoor / simulated mode?) the altitude comes from
elsewhere.

**D+ / D−** (same function, two mirrored state machines on the altitude in dm, once per sample, plus
`FUN_42140aac` / `FUN_42140bc4` that close the open segment at a lap or a pause):

- idle: follow the lowest altitude; when the altitude is **more than 0.5 m** above it, enter *climbing* and start
  the segment **at the current altitude** (the first 0.5 m of each climb is not counted);
- climbing: follow the top; the live total is the committed total + (top − start) if that is **≥ 1.1 m**; when
  the altitude falls **0.5 m** below the top, commit the segment if ≥ 1.1 m and go back to idle;
- the same for the lap totals; the descent machine runs only once the base altitude is set.

Also: VAM from 1 s differences above 0.1 m (× 3600), and an instant value written to a ring buffer.

**Checked against the vendor files**: `tools/reverse/vendor_dplus.py` replays this state machine on the recorded
`enhanced_altitude` and gets the vendor `total_ascent` / `total_descent` to **1.0 % / 0.6 % (median)** over 23
rides. The rest comes from the 0.2 m rounding of the file. A plain 2 m hysteresis on the same series gives 3 %
less in total, up to 10 % on a ride.

Black-box observations that led there: records hold `enhanced_altitude` (+ `altitude`), `grade`, `temperature`,
**no pressure**; altitude in 0.2 m steps, little smoothing (RMS of the second difference 0.15–0.25 m at 1 Hz);
altitude jumps > 5 m only across pauses or power-offs, plus one −12 m in 1 s mid-ride (1780739728, 5 h 30 min).

**Tools**: `tools/reverse/esp_image_elf.py` (app image → ELF), Ghidra 12.1.4 headless with
`tools/reverse/ghidra/AddRomSymbols.java` (ESP32-S3 ROM names from esp-rom-sys `ld/esp32s3/rom/*.ld`: the soft-float
calls read as `__divdf3`...) and `tools/reverse/ghidra/DecompileRefs.java` (decompiles the users of strings or literal
pool constants). Ghidra prints the float literals of the pools wrong when they are integers (`1.4013e-45` is 1):
read the pool words (`DAT_...`) from the image.

## nRF firmware (2026-10-21)

The nRF's application is in the vendor storage partition (`/APP/*/N51_V1.602_A.bin`, XOR-encrypted): nRF52810,
ANT SoftDevice, barometer SPL06-001 whose pressure it filters (trimmed mean + Butterworth, ~2 s late) before the
`F1 03` frames. All in **docs/NRF_FIRMWARE.md**; extraction with `tools/reverse/nrf_image.py`.

## Safety rules

- **Never** write eFuses (irreversible).
- Do not overwrite the bootloader (0x0) or the partition table (0x8000).
- Only write ota_0 (+ otadata), keep the vendor firmware in ota_1.
- Keep the full flash dump before any write.
