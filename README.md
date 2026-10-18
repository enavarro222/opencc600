# opencc600

Open firmware for the **GEOID CC600** bike computer (ESP32-S3; a Magene C606 board without eMMC and without touch).

Status: reverse engineering and product definition done, code to come. The hardware drivers will come from
[c606-oss](https://github.com/HighwayStar/c606-oss) (GPL-3.0).

## Docs
- [docs/PRODUCT.md](docs/PRODUCT.md): decisions, constraints, product questions, technical plan, flash / rollback workflow.
- [docs/REVERSE_NOTES.md](docs/REVERSE_NOTES.md): reverse engineering notes (hardware, eFuses, partitions,
  pinout read over JTAG, first run of c606-oss).
- [docs/pins_live.txt](docs/pins_live.txt): raw output of `tools/read_pins.py` on the vendor firmware.

## Tools
- `tools/read_pins.py`: reads the pin routing of a running ESP32-S3 over its built-in USB-JTAG
  (Espressif OpenOCD), read-only.

## License
GPL-3.0-or-later, see [LICENSE](LICENSE).
