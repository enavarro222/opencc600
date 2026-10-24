"""The climbs-page distance on the emulated CC600's actual display."""
import struct
import subprocess
import zlib

import sim

if why := sim.missing():
    import pytest

    pytest.skip(why, allow_module_level=True)


def png_rgb(path):
    """Read the RGB PNG written by esp32sim (its rows use the no-filter PNG encoding)."""
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    pos = 8
    compressed = bytearray()
    while pos < len(data):
        size = struct.unpack_from(">I", data, pos)[0]
        kind = data[pos + 4:pos + 8]
        chunk = data[pos + 8:pos + 8 + size]
        pos += size + 12
        if kind == b"IHDR":
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", chunk)
            assert (depth, color, compression, filtering, interlace) == (8, 2, 0, 0, 0)
        elif kind == b"IDAT":
            compressed.extend(chunk)
        elif kind == b"IEND":
            break
    raw = zlib.decompress(compressed)
    stride = width * 3
    rows = []
    for y in range(height):
        start = y * (stride + 1)
        assert raw[start] == 0, "unexpected PNG row filter"
        rows.append(raw[start + 1:start + 1 + stride])
    return width, height, rows


def make_route(tmp_path):
    gpx = sim.ROOT / "crates/ui/tests/fixtures/climbs.gpx"
    route = tmp_path / "climbs.route"
    built = sim.ROOT / "target/release/route-tool"
    cmd = [str(built)] if built.exists() else ["cargo", "run", "-q", "--release", "-p", "route-tool", "--"]
    subprocess.run(cmd + ["build", str(gpx), str(route)], check=True, cwd=sim.ROOT, capture_output=True)
    return route.read_bytes()


def climb_page(tmp_path, route, position, name):
    out = tmp_path / name
    out.mkdir()
    screenshot = out / "screen.png"
    script = sim.Script(start=3.0)
    script.at("ble connect")
    script.at("ble mtu 247")
    script.at(f"ble discover", then=0.3)
    script.at(f"ble subscribe {sim.TX_CCCD}", then=0.3)
    script.write(sim.HELLO, then=0.5)
    script.send_file(sim.ROUTE, route)
    script.at("serial key 2 click", then=1.0)  # start the ride
    script.at("serial key 1 click", then=0.5)  # Map -> Essential
    script.at("serial key 1 click", then=0.5)  # Essential -> Climbs
    script.at("serial screen ?", then=0.5)
    if position is not None:
        lat, lon = position
        script.at(f"serial fix {lat} {lon} 3000 0", then=0.5)
    else:
        script.t += 0.5
    run = sim.run(script, out, sim.flash_image(out), max_seconds=script.t + 1, tft_png=screenshot, stop_after=0.5)
    assert not run.lines(r"PANIC|panicked"), "the firmware panicked: see run.log"
    assert run.lines(r"screen Ride Climbs"), f"did not reach the climbs page: see {out / 'run.log'}"
    return png_rgb(screenshot)


def non_background_pixels(image, box=(190, 117, 228, 164)):
    width, height, rows = image
    x0, y0, x1, y1 = box
    assert x1 <= width and y1 <= height, (width, height)
    pixels = [tuple(rows[y][x * 3:x * 3 + 3]) for y in range(y0, y1) for x in range(x0, x1)]
    background = max(set(pixels), key=pixels.count)
    return sum(pixel != background for pixel in pixels)


def test_climb_distance_needs_an_on_route_fix_on_the_emulated_device(tmp_path):
    route = make_route(tmp_path)
    on_route = climb_page(tmp_path, route, (434_500_000, 20_500_000), "on-route")
    no_fix = climb_page(tmp_path, route, None, "no-fix")
    off_route = climb_page(tmp_path, route, (434_500_000, 21_000_000), "off-route")

    assert non_background_pixels(on_route) > 0, "the on-route fix should draw the distance"
    assert non_background_pixels(no_fix) == 0, "the lost fix must hide the distance"
    assert non_background_pixels(off_route) == 0, "the off-route fix must hide the distance"
