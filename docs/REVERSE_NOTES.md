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

## Safety rules

- **Never** write eFuses (irreversible).
- Do not overwrite the bootloader (0x0) or the partition table (0x8000).
- Only write ota_0 (+ otadata), keep the vendor firmware in ota_1.
- Keep the full flash dump before any write.
