# The nRF co-processor's firmware (2026-10-21)

What we know of the vendor's nRF firmware, from its update image. We never flash the nRF: this is to understand
what reaches the ESP32 (link protocol in REVERSE_NOTES.md, "nRF link on the CC600").

## The image

- **Where**: the vendor storage partition (backup `dump/<mac>/storage.orig.bin`, littlefs) holds the update the ESP32
  pushes to the nRF (vendor log lines "SendtoSlave OTA_UPG_CMD", "Slave MCU Upgrade Success!"):
  `/APP/20260415221525_3592/N51_V1.602_A.bin`, 70 394 bytes. Our firmware never touches `/APP`.
- **Extraction**: `tools/reverse/nrf_image.py dump/<mac>/storage.orig.bin dump/nrf` (or `--file N51_*.bin`) writes the plain
  application `N51_V1.602_A_app_0x12000.bin`. `dump/nrf/` is not in git.
- **Format**: a 128-byte header XORed with `9A 92 45 42`, then the application XORed with `DA DC 05 0C` (both repeat
  every 4 bytes; the body key came from the vector table and the strings). Header, little endian:

  | Offset | Value | Meaning |
  |---|---|---|
  | 0x00 | `AA555AA5` | magic |
  | 0x04 | `4E4089FB` | unknown: not a CRC32, CRC16 nor a sum of the body (tried) |
  | 0x08 | 0x400 | unknown (a page size?) |
  | 0x0C | 0x12000 | application address |
  | 0x10 | 0x16000 | unknown (room for the application?) |
  | 0x14 | 0x11278 (70 264) | application size; the body has 2 more bytes (a check?) |
  | 0x18 | 0x80 | header size |
  | 0x1C | 0x1F110 | unknown |
  | 0x2C | `NRF52810_APP` | chip, text |
  | 0x4C | `FV1.000.00` | firmware version, text |
  | 0x58 | `HV001` | hardware version, text |
  | 0x68 | `Nov 10 2025 09:13:38` | build date, text |

  The file name says V1.602 and the header FV1.000.00, while the nRF on our device answers 1.2.16 (`cmd 01`): the
  numbering does not match, so we are not sure this is the exact version running.
- **The application**: nRF52810 (192 KB flash, 24 KB RAM), at 0x12000 after the SoftDevice. Initial SP
  0x200058b0, reset handler 0x12481. Nordic SDK (nrfx drivers, app_timer, nrf_fstorage, RTT log backend, Magene
  code under `..\magene\mg_apps\application\`). Ghidra: raw binary, `ARM:LE:32:Cortex`, base 0x12000; the project
  is in `dump/nrf/ghidra` (`tools/reverse/ghidra/DecompileAt.java` decompiles the functions at given addresses and their
  callees, `tools/reverse/ghidra/Callers.java` prints the instructions before each call, i.e. the arguments).

## What it does (strings and the parts decompiled)

- **SoftDevice: ANT only** (`nrf_sdh_ant`, `sd_ant_enable`; no BLE strings: BLE is the ESP32's). An ANT license
  key string is in the image (not copied here). ANT+ profiles: heart rate (`ant_hrm_page_0..7`), speed / cadence
  (`ant_bcs_*`, `ant_bsc_disp_open`), power (`ant_bpwr_*`, torque pages), FE-C trainers, radar (`ant_rdr_*`),
  bike lights (`ant_light_*`), shifting (`ant_shft_*`) and Shimano Di2 (`ant_di2_*`); scan with RSSI ("scan
  result deviceNum-transType"), "No ANT Device!!!".
- **Battery and charger**: ADC (`nrfx_saadc`), "Battery voltage value:%d, ChargeStatus:%d", percentage smoothing
  ("PercentOut", "Kcoefficient"), `NO_CHARGE` / `CHARGING` / `BAT_TEMP_ERROR`, "Low Power ,PowerON Fail !".
- **Keys and power**: "ButtonId:%d, Event:%d", long press timeouts, key combinations ("ButtonCombinationEvent"),
  power-on reasons (`SetStartMode`, `PowerOffNoChargeHandle`, `RestartMainBordHandle`, "PowerOnRestartFormtimer").
- **ESP32 control**: "Enter EspProgram Mode, Boot Pin set!" (the nRF can put the ESP32 in its ROM bootloader),
  "Enter Reset Handle!", "Enter Check Mode!" (factory?), `GnssRtcReset`.
- **ESP32 program mode** (`FUN_00013a34`, checked 2026-10-22): keys 1 and 2 (the bottom ones) held together ~3 s
  drive P0.24 high (the ESP32 boot pin), reset the ESP32 (output 5) and set config word 7 to 0xa5a5a5a5: every
  ESP32 reset then ends in the ROM bootloader (`boot:0x2`, black screen). In that mode the code beeps every ~3 s
  with no key down, a long press of key 0 released (event 5) resets the nRF (`AIRCR`), and keys 1 and 2 held more
  than 24 s ("Reset Handle") reset it after 1 s; at boot the nRF writes its default config (word 7 cleared).
  Seen on the device: a long press of key 0 alone did nothing; keys 1 and 2 held ~30 s then released started the
  beeps, the ESP32 still in its bootloader; a long press of key 0 after that and the ESP32 booted. Why both were
  needed is not understood.
- **Buzzer**: "set_buzzer_count:%d", "Buzzer Count: %d".
- **RTC**: "current time_stamp:%s" (date formatting).
- **UART to the ESP32**: `mg_drv_uart.c`, "UART_COMMUNICATION_ERROR".

## The barometer and its filter

- **Sensor**: Goertek **SPL06-001** (chip id 0x10) or **SPA06-003** (0x11), on I2C (`nrfx_twim`); set up in
  `FUN_000192e8` ("spl06001 baro ok" / "spl0601&spa06003 baro chip id error"). SPL06: pressure 16 Hz × 32
  oversampling, temperature 16 Hz × 32; SPA06: 32 Hz × 8. Continuous mode (`MEAS_CFG` = 7), result shift bits set
  for the oversampling (`CFG_REG`), no FIFO. Calibration coefficients read in `FUN_00018de8`. The chip itself has
  no IIR filter.
- **Sampling**: an app_timer of 0x666 ticks; the RTC prescaler is 1 (`FUN_0001848c` → `FUN_0001fc84(1)`), 16 384
  Hz, so **every 100 ms (10 Hz)**. Handler `FUN_00020158`:
  1. reads and compensates temperature and pressure;
  2. **trimmed mean**: the last 4 pressures + the new one, minus the min and the max, / 3 (the first 4 samples
     pass as they are);
  3. **2nd-order Butterworth low-pass** (`FUN_00015cb4`, direct form I, on the pressure minus its first value):
     b = [0.0013487119, 0.002697423, 0.0013487119], a = [1, −1.893464, 0.8988589]: at 10 Hz, fc ≈ 0.12 Hz,
     **lag ≈ 1.9 s on a steady climb**, step response 50 % at 1.9 s and 90 % at 3.5 s, 4 % overshoot;
  4. a validity flag: pressure within 91 300–131 300 Pa and temperature within −40–85 °C;
  5. altitude `44330 × (1 − (p / 100 / 1013.25)^0.190295)` × 10 (dm), as in the ESP32 vendor firmware.
- The `F1 03` frames (5 Hz) carry the filtered pressure (0.01 Pa): **what the ESP32 gets is already ~2 s late**
  (consistent with the ride below: our pressure lags our own GNSS altitude by ~2 s).

## Measured on a ride (2026-10-21)

CC600 (our firmware, `trip_log on`), Bryton Rider 750 and Forerunner 955 on the same bike, 35 min, ~190 m of
climbing; files in `dump/ride_1021/`. The three tracks agree in time to 1 s. Lags of the vertical speed (over
10 s, cross-correlated; the Garmin's barometer drifts but its changes are the quickest):

| Signal | Lag behind the Garmin |
|---|---|
| Bryton altitude | 2 s |
| CC600 GNSS altitude | 3 s |
| CC600 pressure from the nRF | 5 s |
| CC600 FIT altitude (our Kalman) | 5 s (≤ 1 s more than the pressure) |

Of the CC600's 5 s, ~2 s are the nRF filter; the rest is not explained (the case's vent? to measure with a step
in pressure, e.g. a bag squeezed around the unit). The displayed gradient adds its own window on top: over 100–200 m back, it lagged the Bryton's by ~19 s on this
ride; over the last 40 m since (`GRADE_BACK_CM`), ~9 s when the ride is replayed.
