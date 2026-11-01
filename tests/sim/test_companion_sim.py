"""The companion link end to end on the emulated device (sim.py): the app's hello, a route then its background map
sent as the app sends them, what the device then says of its room and what it logs.

    uv run --project tests/hil pytest tests/sim
"""
import gzip
import json
import struct
import subprocess
from pathlib import Path

import pytest

import sim

if why := sim.missing():
    pytest.skip(why, allow_module_level=True)


def gpx(n: int = 150) -> str:
    """3 km northwards, a point every 20 m, weaving by 15 m (every point stays after the 3 m simplification)."""
    pts = "".join(
        f'<trkpt lat="{44.0 + i * 20 / 111_320:.7f}" lon="{1.7 + (15 if i % 2 else -15) / 80_300:.7f}">'
        "<ele>200</ele></trkpt>"
        for i in range(n)
    )
    return f'<gpx><trk><name>Sim</name><trkseg>{pts}</trkseg></trk></gpx>'


def route_tool(*args: str) -> None:
    """route-tool, built already when it can (`cargo build --release -p route-tool`), else through cargo."""
    built = sim.ROOT / "target/release/route-tool"
    cmd = [str(built)] if built.exists() else ["cargo", "run", "-q", "--release", "-p", "route-tool", "--"]
    subprocess.run(cmd + list(args), check=True, cwd=sim.ROOT, capture_output=True)


def osm() -> dict:
    """Overpass JSON (`out geom`): two roads along the route, a path, a forest."""
    line = lambda dx, n=40: [{"lat": 44.0 + i * 80 / 111_320, "lon": 1.7 + dx / 80_300} for i in range(n)]
    ring = [{"lat": 44.005, "lon": 1.705}, {"lat": 44.005, "lon": 1.71}, {"lat": 44.015, "lon": 1.71},
            {"lat": 44.015, "lon": 1.705}, {"lat": 44.005, "lon": 1.705}]
    way = lambda tags, geometry: {"type": "way", "tags": tags, "geometry": geometry}
    return {"elements": [
        way({"highway": "tertiary"}, line(120)),
        way({"highway": "track"}, line(-250)),
        way({"highway": "path"}, line(400)),
        way({"natural": "wood"}, ring),
    ]}


@pytest.fixture(scope="module")
def files(tmp_path_factory):
    """The route and its map, both built by route-tool from the same GPX: the map's route CRC matches."""
    d = tmp_path_factory.mktemp("files")
    (d / "sim.gpx").write_text(gpx())
    (d / "osm.json").write_text(json.dumps(osm()))
    route_tool("build", str(d / "sim.gpx"), str(d / "route.bin"))
    route_tool("map", str(d / "sim.gpx"), str(d / "osm.json"), str(d / "map.bin"))
    return (d / "route.bin").read_bytes(), (d / "map.bin").read_bytes()


def device_infos(run: sim.Run) -> list[tuple[int, int, int]]:
    """Every `DeviceInfo` the device answered: route_max, map_max, free."""
    return [struct.unpack("<III", p[:12]) for op, _, p in run.notifications() if op == sim.DEVICE_INFO]


def test_a_route_and_its_map_sent_by_the_app(files, tmp_path):
    route, map_ = files
    script = sim.Script(start=3.0)  # the device advertises within a second
    script.at("ble connect")
    script.at("ble mtu 247")
    script.at("ble discover", then=0.3)
    script.at(f"ble subscribe {sim.TX_CCCD}", then=0.3)
    script.write(sim.HELLO, then=0.5)
    script.send_file(sim.ROUTE, route)
    script.send_file(sim.MAP, map_)
    script.write(sim.HELLO, then=0.5)
    run = sim.run(script, tmp_path, sim.flash_image(tmp_path), max_seconds=script.t + 10)

    assert not run.lines(r"PANIC|panicked"), "the firmware panicked: see run.log"
    assert run.lines(rf"characteristic .*handle=0x{sim.RX:04x} .*uuid={sim.RX_UUID}"), "the GATT table moved"
    assert run.lines(r"\[ble\] .* mtu 247")
    # stored and taken: the map is the route's (its CRC), the device says so
    assert run.lines(r"ble: app Route received")
    assert run.lines(r'route: loaded "Sim"')
    assert run.lines(r"ble: app Map received")
    assert run.lines(rf"map: {len(map_)} bytes"), "the map was not taken as the route's"
    # every chunk acked, no error answered
    ops = [op for op, _, _ in run.notifications()]
    assert sim.ERROR not in ops, f"the device answered an error: {run.notifications()}"
    assert ops.count(sim.OK) == 2, "each TransferEnd answers Ok"
    # the room before and after: the files took some
    infos = device_infos(run)
    before, after = infos[0], infos[-1]
    assert before[1] > len(map_) and after[1] > 0
    assert after[2] < before[2], f"the free bytes did not go down: {before} -> {after}"


def dense_paths(lines: int = 400) -> dict:
    """Overpass JSON: `lines` paths along the route, weaving by 6 m every 8 m (the 2 m simplification keeps every
    point): ~1.1 KB of map each."""
    ways = []
    for k in range(lines):
        dx = -1400 + k * 2800 / lines
        geometry = [{"lat": 44.0 - 0.002 + i * 8 / 111_320, "lon": 1.7 + (dx + (6 if i % 2 else -6)) / 80_300}
                    for i in range(470)]
        ways.append({"type": "way", "tags": {"highway": "path"}, "geometry": geometry})
    return {"elements": ways}


def test_the_permanent_atlas_survives_route_removal_and_a_device_reboot(files, tmp_path):
    """The real firmware acknowledges the stored atlas and keeps it independently of route/map lifecycle."""
    route, map_ = files
    atlas = (Path(__file__).parent / "fixtures/atlas.ocat").read_bytes()
    script = sim.Script(start=0.0)
    script.at("board usb on", then=3.0)

    def connect():
        script.at("ble connect")
        script.at("ble mtu 247")
        script.at(f"ble subscribe {sim.TX_CCCD}", then=0.3)

    connect()
    script.write(sim.HELLO, then=0.5)
    script.send_file(sim.ATLAS, atlas)
    script.write(sim.HELLO, then=0.5)
    script.send_file(sim.ROUTE, route)
    script.send_file(sim.MAP, map_)
    script.write(sim.HELLO, then=0.5)
    script.at("serial route clear", then=1.5)
    script.write(sim.HELLO, then=0.5)
    script.at("ble disconnect", then=0.5)
    script.at("press key0 1500", then=8.0)
    connect()
    script.write(sim.HELLO, then=1.0)
    run = sim.run(script, tmp_path, sim.flash_image(tmp_path), max_seconds=script.t + 10)
    assert not run.lines(r"PANIC|panicked|alloc"), "firmware panicked or ran out of memory: see run.log"
    errors = [n for n in run.notifications() if n[0] == sim.ERROR]
    assert not errors, f"device refused an installation: {errors}"
    assert run.lines(r"ble: app Atlas received"), "atlas was not durably acknowledged"
    assert run.lines(r"route: removed"), "the active route was not removed"
    assert run.lines(r"chip reset .*cause 0x1\b"), "the reboot did not happen"
    infos = [struct.unpack("<IIIII", p[:20]) for op, _, p in run.notifications() if op == sim.DEVICE_INFO]
    assert len(infos) == 5, f"missing device info: {infos}"
    assert infos[0][3:] == (512 * 1024, 0)
    assert all(i[4] == len(atlas) for i in infos[1:]), "atlas lost on route change/removal or reboot"


def announced_map_max(tmp_path, image) -> int:
    """The largest map the device says it takes (its first `DeviceInfo`)."""
    script = sim.Script(start=3.0)
    script.at("ble connect")
    script.at("ble mtu 247")
    script.at(f"ble subscribe {sim.TX_CCCD}", then=0.3)
    script.write(sim.HELLO, then=0.5)
    (tmp_path / "hello").mkdir()
    return device_infos(sim.run(script, tmp_path / "hello", image, max_seconds=script.t + 2))[0][1]


def test_the_largest_map_twice_in_the_cc600_psram(files, tmp_path):
    """A map as big as the device says it takes, then another one: the CC600 has 2 MB of PSRAM (the framebuffer takes
    150 KB), the old map is shown while the new one comes, and the receiving buffer must not grow by copies. Whatever
    the device announces, it holds, with a full permanent atlas also installed. Replacing the atlas must fit too."""
    route, _ = files
    atlas = gzip.decompress((Path(__file__).parent / "fixtures/atlas-max.ocat.gz").read_bytes())
    assert 0.99 * 512 * 1024 < len(atlas) <= 512 * 1024
    image = sim.flash_image(tmp_path)
    room = announced_map_max(tmp_path, image)
    (tmp_path / "sim.gpx").write_text(gpx())
    # ~1.1 KB of map a path: as many as fill the room, the last ones dropped until it fits
    lines = room // 1150
    while True:
        (tmp_path / "osm.json").write_text(json.dumps(dense_paths(lines)))
        route_tool("map", str(tmp_path / "sim.gpx"), str(tmp_path / "osm.json"), str(tmp_path / "big.bin"))
        big = (tmp_path / "big.bin").read_bytes()
        if len(big) <= room:
            break
        lines -= max(1, (len(big) - room) // 1000)
    assert len(big) > room * 0.9, f"the map ({len(big)} bytes) is far under the room ({room})"

    script = sim.Script(start=3.0)
    script.at("ble connect")
    script.at("ble mtu 247")
    script.at(f"ble subscribe {sim.TX_CCCD}", then=0.3)
    script.send_file(sim.ATLAS, atlas, chunk=230, every=0.012)
    script.send_file(sim.ROUTE, route)
    for _ in range(2):
        script.send_file(sim.MAP, big, chunk=230, every=0.012)
    script.send_file(sim.ATLAS, atlas, chunk=230, every=0.012)
    run = sim.run(script, tmp_path, image, max_seconds=script.t + 10)

    assert not run.lines(r"PANIC|panicked|alloc"), "the firmware ran out of memory or panicked: see run.log"
    assert len(run.lines(rf"map: {len(big)} bytes")) == 2, f"each map of {len(big)} bytes is taken"
    assert len(run.lines(r"ble: app Atlas received")) == 2, "both full atlases are stored"
    ops = [op for op, _, _ in run.notifications()]
    errors = [n for n in run.notifications() if n[0] == sim.ERROR]
    assert not errors, f"the device answered an error: {errors}"
    assert ops.count(sim.OK) == 5, "the route, both maps and both atlases end with Ok"
