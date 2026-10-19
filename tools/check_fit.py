#!/usr/bin/env -S uv run -q --with garmin-fit-sdk python3
"""Decode FIT files with the official Garmin FIT SDK and report errors (integrity, CRC, profile).

Usage: tools/check_fit.py FILE.fit [...] [-v]   (-v prints every message)
"""
import sys

from garmin_fit_sdk import Decoder, Stream


def check(path: str, verbose: bool) -> bool:
    stream = Stream.from_file(path)
    decoder = Decoder(stream)
    if not decoder.is_fit():
        print(f"{path}: not a FIT file")
        return False
    if not decoder.check_integrity():
        print(f"{path}: integrity check failed (header or CRC)")
        return False
    # is_fit / check_integrity consume the stream: decode from a fresh one
    messages, errors = Decoder(Stream.from_file(path)).read(convert_datetimes_to_dates=False)
    counts = {k: len(v) for k, v in messages.items()}
    print(f"{path}: {counts}")
    for e in errors:
        print(f"  error: {e}")
    if verbose:
        for kind, msgs in messages.items():
            for m in msgs[:3]:
                print(f"  {kind}: {m}")
    return not errors


if __name__ == "__main__":
    verbose = "-v" in sys.argv
    paths = [a for a in sys.argv[1:] if a != "-v"]
    sys.exit(0 if all([check(p, verbose) for p in paths]) else 1)
