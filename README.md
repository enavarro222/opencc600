<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/logo-wide-night.svg">
    <img src="docs/logo-wide.svg" alt="OpenCC600" width="360">
  </picture>
</p>

# opencc600

Open firmware for the **GEOID CC600** bike computer, also running on the **GEOID CC700**. Written in Rust
(`no_std`, esp-hal + Embassy); the hardware layer is a rewrite based on
[c606-oss](https://github.com/HighwayStar/c606-oss). The companion app is
[Musette](https://github.com/enavarro222/musette).

It installs next to the vendor firmware, which stays on the device: one command brings it back.

> **Use at your own risk.** This is an unofficial hobby project, not affiliated with or endorsed by GEOID or
> Magene. Flashing it may void your warranty and could leave your device unusable; rides may be lost and figures
> may be wrong. Keep the backups `tools/flash.py` makes. The software comes with no warranty (GPL-3.0,
> sections 15–16): the authors are not liable for any damage to your device, your data or anything else.

## Features

| Ride page | Altitude | Map | In a climb | Night |
|:-:|:-:|:-:|:-:|:-:|
| ![Essential page](docs/screens/ride-page1.png) | ![Altitude page](docs/screens/ride-page2.png) | ![Map page](docs/screens/route-map-1km.png) | ![In a climb](docs/screens/climbs-in.png) | ![Night theme](docs/screens/ride-page1-night.png) |

- **Ride pages picked for you** from what the device sees at the start (route, power meter, heart rate): no page
  editor, large figures, a night theme.
- **Accurate altitude and D+** from the barometer, calibrated on the GPS.
- **Routes from your phone**: a map turned to your heading, an off-route alert, the climbs ahead with the distance,
  D+ and time left to the top.
- **BLE sensors**: heart rate, speed / cadence, power with its zones and calibration.
- **Rides that survive a power cut**, saved as standard FIT files, resumable after a power off.
- **Companion app** [Musette](https://github.com/enavarro222/musette): routes, settings and rides synced over BLE.

All the screens and details: [docs/FEATURES.md](docs/FEATURES.md). Clickable mock-up of the whole UI (in French for
now): **https://enavarro222.github.io/opencc600/prototype/**

The real firmware in your browser, on an emulated device (in French too): **https://enavarro222.github.io/opencc600/demo/** ([how it works](docs/demo/README.md))

## Supported devices

| Device | Status |
|---|---|
| GEOID CC600 | **Supported**, ridden |
| GEOID CC700 | **Runs, not ridden yet**: same board, larger screen, same build ([docs/CC700.md](docs/CC700.md)) |
| GEOID CC700 Pro, Magene C606 / C606 Pro / C706 | **Not supported** (touch screen, eMMC): see [c606-oss](https://github.com/HighwayStar/c606-oss) |

Each was tested on one unit only.

## Install

Plug the device in over USB-C. Everything goes through that cable; no button to press.

1. **Dump the whole flash first.** It is the only way back to a stock device:
   `esptool -p /dev/ttyACM0 -b 921600 read-flash 0 ALL dump/flash_full.bin` (about 90 s).
2. Build the image ([docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#build-and-flash)), then flash it:
   `tools/flash.py dump/opencc600.bin`. It writes the OTA slot the vendor firmware does not use, backs up then
   erases the vendor's storage partition, and keeps every backup in `dump/<mac>/`. `tools/flash.py --status` shows
   the device's state without writing anything.
3. **Back to the vendor firmware**: `tools/flash.py --restore-vendor --storage dump/<mac>/storage.orig.bin`.

## Roadmap

MVP status in [docs/PRODUCT.md](docs/PRODUCT.md#mvp). Next: battery life ([docs/POWER.md](docs/POWER.md)), faster
GPS fix, more beeps and key lock, faster ride sync, ANT+ with real sensors. Later: an OSM background map around the
route, Wi-Fi upload, a web tool to send routes from a browser.

Out of scope: global maps and on-device routing.

## Documentation

- [docs/FEATURES.md](docs/FEATURES.md): every feature, with its screens.
- [docs/PRODUCT.md](docs/PRODUCT.md): principles, UI rules, ride pages, features, MVP status, open questions.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): code layout, build, tools.
- [docs/DEV_PROCESS.md](docs/DEV_PROCESS.md): how it is tested (PC, device over USB, field), flash and rollback.
- [docs/ALTITUDE.md](docs/ALTITUDE.md): barometric altitude and D+.
- [docs/POWER.md](docs/POWER.md): battery life.
- [docs/REVERSE_NOTES.md](docs/REVERSE_NOTES.md): the hardware, as reverse engineered from the CC600.
- [docs/CC700.md](docs/CC700.md): the CC700 against the CC600.
- [docs/NRF_FIRMWARE.md](docs/NRF_FIRMWARE.md): the nRF co-processor's firmware.

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). Except the font data: Barlow Condensed, Copyright 2017 The Barlow
Project Authors, SIL Open Font License 1.1 ([crates/ui/fonts/OFL.txt](crates/ui/fonts/OFL.txt)), in
`crates/ui/fonts/` and the bitmaps generated from it in `crates/ui/src/fonts/` (`generated.rs`, `*.bin`).
