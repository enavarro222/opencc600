# opencc600 — power and battery life

Battery life is a core requirement of a bike computer: a firmware that records perfectly but dies after 6 hours
is useless on a long ride. This document collects what consumes, what we know, how we measure, and the levers.
Context: [PRODUCT.md](PRODUCT.md), hardware: [REVERSE_NOTES.md](REVERSE_NOTES.md).

## Target

- **At least the vendor firmware's battery life** in the same use (GNSS recording, screen on, sensors).
  *(To find: the CC600 rated autonomy, and to measure: the vendor firmware on our unit, same scenario.)*
- Battery: **1800 mAh** Li-ion. Rough budget: 20 h needs ~90 mA average, 30 h ~60 mA, 40 h ~45 mA
  (ignoring the end-of-discharge margin).
- Off / standby: the nRF stays powered when the ESP32 is off; its drain sets how long the unit can sit on a shelf.
  *(To measure.)*

## Who consumes

Only order-of-magnitude figures from datasheets until we measure on this board: nothing below is a CC600
measurement unless marked so.

| Consumer | Controlled by | What we know | Levers |
|---|---|---|---|
| ESP32-S3 CPU (240 MHz, dual core) | us | runs at max clock, **never sleeps** today | async idle, lower clock, light sleep |
| 2 MB quad PSRAM | us | on, 80 MHz | keep it, but avoid needless traffic (framebuffer copies) |
| Backlight (GPIO45, LEDC 20 kHz) | us | from the ambient light sensor (`auto_backlight`, `ui::light`: 10 % dark to 100 % bright), else the `backlight` setting | curve, timeout, dimming |
| ST7789 panel | us | full frame redrawn when the view changes, ≤ 5 Hz | redraw only on change, 1 Hz data screens, panel sleep / idle mode |
| AG3352Q GNSS | us (power through the nRF, `$PAIR`) | 4 constellations, 1 Hz, ~2.6 KB/s of NMEA | constellations, sentences, power modes, off when not recording |
| nRF co-processor | its own firmware | sensor stream 5 Hz once started, ANT+ | little; its UART traffic wakes the ESP32 |
| BLE (ESP32) | us | sensors and the Musette link, always on | connection parameters, scan windows |
| Wi-Fi (ESP32) | us | not used yet | only for the sync at home, short sessions |
| Flash writes | us | ride appends, sector erases | batch writes (trade-off with crash loss) |
| Buzzer | us (through the nRF) | short sounds | negligible |

## What we already know

- The ESP32-S3 runs at **240 MHz**. Embassy already idles the CPU between events (`waiti`), but there is no
  light sleep and no clock scaling.
- The screen is redrawn whenever what it shows changes (up to 5 Hz), and the full 150 KB frame is sent each time
  (11 ms of bus + CPU copy per frame, see REVERSE_NOTES.md).
- The GNSS output is trimmed to what we use (GGA, RMC, GSV every 5 s); the Airoha `$PAIRSAT` reports (~1.2 KB/s)
  are still sent and thrown away.
- The nRF powers the GNSS: it can be left **off** after a power cycle (seen once) and comes back with a cold
  start. The vendor parks the receiver in a low-power RTC mode with `$PAIR650,0` (c606-oss) instead of cutting
  it, which keeps ephemerides for a fast hot start.
- The vendor keeps A-GNSS data in `/CONFIG/<t>_agnss.bin` (157 KB): assisted start = less time with the GNSS at
  full power before a fix.
- The ambient light sensor works (`F1 04`, raw level ~1000 indoors at night): automatic backlight is possible.
- The board temperature reads ~30 °C in a 20–22 °C room with USB plugged in and the CPU never sleeping: the
  heat is wasted energy, and a first rough indicator.

## How we measure

The case is not opened, so no current probe on the rails, and **plugged into USB the battery charges**: the unit
must run on battery during a measurement. Simple ways, cheapest first:

1. **Battery run, read back afterwards (reference method, no hardware)**: the firmware appends one line a minute
   to `/power/<run>.csv` on our littlefs: uptime, battery millivolts and percent from the nRF, the configuration
   under test (backlight, GNSS mode, CPU clock...). Charge to 100 %, unplug, let it run; plug back in and read the
   file over the serial link to plot it. The ride-store guarantee applies: the last acknowledged line survives
   the battery dying. Below 3 % the firmware writes an end-of-run line and powers off cleanly (`power_off`).
   - A **full discharge** gives the real battery life, nRF included, with no assumption.
   - A **partial run** (100 % → 70 %) is enough to compare two configurations, less precisely (the nRF percent
     is coarse and the voltage curve is not linear).
2. **USB power blocker (live logs on battery, ~5–10 €)**: an adapter that cuts the USB +5 V and keeps D+/D−
   ("USB power blocker", "data only", e.g. PortaPow), or a cable with its VBUS wire cut. The unit then runs on
   battery while `/dev/ttyACM0` still carries the logs: ideal for live A/B comparisons and for watching a
   discharge. *To check on the CC600: the ESP32-S3 USB-Serial-JTAG should enumerate without VBUS since the chip
   is powered by the battery; quick test: plug through the blocker and look for `/dev/ttyACM0`.*
3. **USB input current with a full battery** *(hypothesis to check)*: once the charger has finished, the USB input
   current may be close to the system draw; a USB power meter would then give a fast A/B comparison. To validate
   against method 1 before trusting it.
4. **Firmware counters**: time spent awake vs idle per executor, redraws per minute, bytes received per link.
   Cheap, always on in debug builds, and they explain *why* a configuration draws more.

Each lever below gets an A/B measurement before it becomes a default. Results go in the table at the end.

## Levers, by expected gain

1. **Backlight**: likely the largest consumer while the screen is lit. Automatic level from the ambient light
   sensor, done; its curve rises with the light, to turn down in daylight if the panel stays readable (*to check:
   is the CC600 panel transflective?*), timeout
   with wake on key press, lower default level, short boost on alerts.
2. **CPU sleep and clock**: let the CPU sleep between events (light sleep with the UART / timer wake-ups that
   the links need), run at 80 or 160 MHz when 240 MHz is not needed (the display flush and FIT writes are short).
   The GNSS DMA keeps receiving while the CPU sleeps; the nRF UART needs its FIFO drained in time
   (~11 ms at 115200 baud).
3. **GNSS configuration** (Airoha `$PAIR` commands, values to check on this receiver):
   - only the sentences we use: **done** (`$PAIR062`: GGA + RMC every fix, GSV every 5 s, ~19 → ~7 sentences/s).
     `$PAIRSAT` and the other Airoha outputs are not controlled by `$PAIR062` (still ~1.2 KB/s, command to find);
   - constellations (`$PAIR066`): e.g. GPS + Galileo vs all four; fewer constellations should save power but
     lengthen the time to fix and degrade it under trees and in valleys. User-selectable, default from measurements;
   - the receiver's own power-saving modes, if any are usable at 1 Hz on a bike;
   - parked in RTC / backup mode (`$PAIR650`) instead of off between rides, plus A-GNSS / EPO, for hot starts.
4. **Display work**: redraw only what changed and at most once per second on data screens (partial windows,
   CASET/RASET already supported), panel sleep (`SLPIN`) when the screen is off.
5. **Radios**: Wi-Fi only for the sync at home, BLE with relaxed connection intervals, ANT+ left on the nRF.
6. **Storage**: keep appends batched (e.g. every 10–30 s); each append costs a flash program and sometimes an
   erase. Balance with the power-cut guarantee (what is buffered in RAM is lost on a crash).
7. **Standby**: auto power-off after inactivity when not recording, through the nRF (`power_off`), never during
   a ride.

## Open questions

- Vendor battery life on this unit, same scenario (reference to beat).
- nRF and GNSS current in each state; whether the nRF can lower its sensor stream rate.
- Is method 2 (USB current at full charge) reliable on this charger?
- Is the panel readable in sunlight with the backlight off? (PRODUCT.md, open questions)
- Ambient light sensor range: tag 600 seems to saturate near 2600 under a phone torch; what does it read in
  direct sunlight? (PRODUCT.md, open questions)
- Is the nRF power-on command every 5 s needed? (PRODUCT.md, open questions)
- Which ESP32-S3 sleep mode is compatible with keeping both UART links and the BLE link alive.

## Measurements

| Date | Firmware / config | Method | Result |
|---|---|---|---|
| — | — | — | *none yet* |
