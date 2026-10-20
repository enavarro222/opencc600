#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["bless>=0.2.6"]
# ///
"""A fake BLE cycling sensor on this PC (BlueZ): heart rate (0x180D), speed and cadence (0x1816) and cycling power
(0x1818), with the standard measurements notified every second, for testing the CC600 without a real sensor.

    tools/fake_sensor.py [--name NAME] [--only hr,csc,power] [--seconds N]

Values: heart rate 120..160 bpm, cadence 85..95 rpm, 30 km/h on a 2105 mm wheel, power 180..260 W; they move
slowly so a frozen value on the device shows. Ctrl+C stops it.
"""

import argparse
import asyncio
import math
import struct
import time

from bless import BlessServer, GATTAttributePermissions, GATTCharacteristicProperties

HR, HR_MEASUREMENT = "0000180d-0000-1000-8000-00805f9b34fb", "00002a37-0000-1000-8000-00805f9b34fb"
CSC, CSC_MEASUREMENT, CSC_FEATURE = (
    "00001816-0000-1000-8000-00805f9b34fb",
    "00002a5b-0000-1000-8000-00805f9b34fb",
    "00002a5c-0000-1000-8000-00805f9b34fb",
)
POWER, POWER_MEASUREMENT, POWER_FEATURE = (
    "00001818-0000-1000-8000-00805f9b34fb",
    "00002a63-0000-1000-8000-00805f9b34fb",
    "00002a65-0000-1000-8000-00805f9b34fb",
)
NOTIFY = GATTCharacteristicProperties.read | GATTCharacteristicProperties.notify
READ = GATTCharacteristicProperties.read
READABLE = GATTAttributePermissions.readable

WHEEL_M = 2.105


class Ride:
    """Cumulative wheel and crank revolutions with their event times (1/1024 s), as a sensor counts them."""

    def __init__(self):
        self.t0 = time.monotonic()
        self.wheel = self.crank = 0.0

    def step(self, dt):
        t = time.monotonic() - self.t0
        self.hr = round(140 + 20 * math.sin(t / 30))
        self.rpm = 90 + 5 * math.sin(t / 20)
        self.kmh = 30.0
        self.watts = round(220 + 40 * math.sin(t / 15))
        self.wheel += self.kmh / 3.6 / WHEEL_M * dt
        self.crank += self.rpm / 60 * dt
        # event time of the last whole revolution
        self.wheel_t = round((t - (self.wheel % 1) * WHEEL_M / (self.kmh / 3.6)) * 1024) & 0xFFFF
        self.crank_t = round((t - (self.crank % 1) * 60 / self.rpm) * 1024) & 0xFFFF

    def hr_measurement(self):
        return struct.pack("<BB", 0x00, self.hr)  # flags: u8 bpm

    def csc_measurement(self):
        # flags: wheel (u32 revs, u16 time) and crank (u16 revs, u16 time) data present
        return struct.pack(
            "<BIHHH", 0x03, int(self.wheel) & 0xFFFFFFFF, self.wheel_t, int(self.crank) & 0xFFFF, self.crank_t
        )

    def power_measurement(self):
        # flags: crank revolution data present
        return struct.pack("<HhHH", 0x0020, self.watts, int(self.crank) & 0xFFFF, self.crank_t)


async def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--name", default="OCC Fake")
    ap.add_argument("--only", default="hr,csc,power")
    ap.add_argument("--seconds", type=float, default=0, help="stop after N s (0: never)")
    args = ap.parse_args()
    only = set(args.only.split(","))

    server = BlessServer(name=args.name)
    chars = []
    if "hr" in only:
        await server.add_new_service(HR)
        await server.add_new_characteristic(HR, HR_MEASUREMENT, NOTIFY, None, READABLE)
        chars.append((HR, HR_MEASUREMENT, Ride.hr_measurement))
    if "csc" in only:
        await server.add_new_service(CSC)
        await server.add_new_characteristic(CSC, CSC_MEASUREMENT, NOTIFY, None, READABLE)
        await server.add_new_characteristic(CSC, CSC_FEATURE, READ, struct.pack("<H", 0x03), READABLE)
        chars.append((CSC, CSC_MEASUREMENT, Ride.csc_measurement))
    if "power" in only:
        await server.add_new_service(POWER)
        await server.add_new_characteristic(POWER, POWER_MEASUREMENT, NOTIFY, None, READABLE)
        await server.add_new_characteristic(POWER, POWER_FEATURE, READ, struct.pack("<I", 0x08), READABLE)
        chars.append((POWER, POWER_MEASUREMENT, Ride.power_measurement))
    # bless makes only the first service primary: a sensor's are all primary (the device discovers only those)
    for service in getattr(getattr(server, "app", None), "services", []):
        service._primary = True
    await server.start()
    print(f"advertising {args.name!r}: {', '.join(sorted(only))}", flush=True)

    ride = Ride()
    start = last = time.monotonic()
    try:
        while not args.seconds or time.monotonic() - start < args.seconds:
            await asyncio.sleep(1)
            now = time.monotonic()
            ride.step(now - last)
            last = now
            for service, char, value in chars:
                server.get_characteristic(char).value = bytearray(value(ride))
                server.update_value(service, char)
            print(f"hr {ride.hr} bpm, {ride.rpm:.0f} rpm, {ride.kmh:.0f} km/h, {ride.watts} W", flush=True)
    finally:
        await server.stop()


if __name__ == "__main__":
    asyncio.run(main())
