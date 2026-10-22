"""The link with the companion app (Musette) over BLE, this PC playing the app (bleak, BlueZ): the device advertises
its service, answers `Hello`, reads and changes its settings, receives a route (and resumes it after a dropped link).
The settings and the route on the device before the test are put back afterwards.

The protocol is `protocol-cc600` (github.com/enavarro222/musette): frames `ver | op | seq | len | payload | crc16`.

Needs a Bluetooth adapter on the PC (skipped without one)."""
import asyncio
import struct
import sys
import tempfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import send_route  # noqa: E402
from test_ble_sensors import adapter  # noqa: E402

bleak = pytest.importorskip("bleak")

SERVICE = "18fbbd9a-adda-4ef4-9635-befd2efbf8a3"
RX = "c039ed02-4557-4b35-bc20-5c9320871456"
TX = "1cd21602-4b05-4c39-b9be-8353dece8a3f"

HELLO, GET_SETTINGS, SET_SETTINGS = 0x01, 0x20, 0x21
BEGIN, CHUNK, END = 0x10, 0x11, 0x12
DEVICE_INFO, READY, ACK, RESEND, SETTINGS, OK, ERROR = 0x81, 0x90, 0x91, 0x92, 0xA0, 0xF0, 0xFF
ROUTE = 1
BAD_SETTING = 8


def crc16(data: bytes) -> int:
    return send_route.crc16(data)


def frame(op: int, seq: int, payload: bytes = b"") -> bytes:
    head = struct.pack("<BBBH", 1, op, seq, len(payload)) + payload
    return head + struct.pack("<H", crc16(head))


class App:
    """The app's side: requests answered by `seq`, other frames (transfer acks) in `events`."""

    def __init__(self, client):
        self.client = client
        self.buf = b""
        self.seq = 0
        self.waiting: dict[int, asyncio.Future] = {}
        self.events: asyncio.Queue = asyncio.Queue()

    @classmethod
    async def connect(cls, address: str):
        """Connects to the device at `address` only: another device in range advertises the same service."""
        device = await bleak.BleakScanner.find_device_by_filter(
            lambda d, adv: d.address.upper() == address and SERVICE in [u.lower() for u in adv.service_uuids],
            timeout=20,
        )
        assert device, f"the device plugged in ({address}) does not advertise the companion service"
        client = bleak.BleakClient(device)
        await client.connect()
        app = cls(client)
        await client.start_notify(TX, app._notified)
        print(f"ATT MTU {client.mtu_size}")
        return app

    async def close(self):
        await self.client.disconnect()

    def _notified(self, _, data: bytearray):
        self.buf += bytes(data)
        while len(self.buf) >= 7:
            n = 5 + struct.unpack_from("<H", self.buf, 3)[0] + 2
            if len(self.buf) < n:
                return
            f, self.buf = self.buf[:n], self.buf[n:]
            assert struct.unpack_from("<H", f, n - 2)[0] == crc16(f[: n - 2]), "bad CRC from the device"
            op, seq, payload = f[1], f[2], f[5 : n - 2]
            fut = self.waiting.pop(seq, None)
            if fut and not fut.done():
                fut.set_result((op, payload))
            else:
                self.events.put_nowait((op, payload))

    async def write(self, data: bytes):
        """A frame in writes of the link's size, as the app does (the device reassembles)."""
        piece = max(20, self.client.mtu_size - 3)
        for i in range(0, len(data), piece):
            await self.client.write_gatt_char(RX, data[i : i + piece], response=False)

    async def send(self, op: int, payload: bytes = b"") -> int:
        self.seq = (self.seq + 1) & 0xFF
        await self.write(frame(op, self.seq, payload))
        return self.seq

    async def request(self, op: int, payload: bytes = b"", timeout=5.0):
        fut = asyncio.get_running_loop().create_future()
        self.seq = (self.seq + 1) & 0xFF
        self.waiting[self.seq] = fut
        await self.write(frame(op, self.seq, payload))
        return await asyncio.wait_for(fut, timeout)

    async def send_file(self, kind: int, data: bytes, stop_after: int | None = None) -> tuple[int, int]:
        """Sends `data` as the app does (window, acks, resends); returns (where the device started, bytes sent).
        `stop_after`: give up once that many bytes are sent (the link is then dropped by the caller)."""
        op, p = await self.request(BEGIN, struct.pack("<BIH", kind, len(data), crc16(data)))
        assert op == READY, (op, p)
        start, window = struct.unpack("<IH", p)
        sent, acked, total = start, start, 0
        while acked < len(data):
            while sent < len(data) and sent - acked < window:
                n = min(240, len(data) - sent, window - (sent - acked))
                await self.send(CHUNK, struct.pack("<I", sent) + data[sent : sent + n])
                sent += n
                total += n
                if stop_after is not None and total >= stop_after:
                    return start, total
            op, p = await asyncio.wait_for(self.events.get(), 5)
            if op == ACK:
                acked = max(acked, struct.unpack("<I", p)[0])
            elif op == RESEND:
                sent = acked = struct.unpack("<I", p)[0]
            else:
                raise AssertionError(f"device answered {op:#x} {p.hex()}")
        op, p = await self.request(END)
        assert op == OK, f"route refused: {p.hex()}"
        return start, total


def zigzag_gpx(path: Path, points: int = 1500):
    """A route that keeps every point (a zigzag), tens of KB, from a made-up place (no real position in the tests)."""
    trk = "".join(
        f'<trkpt lat="{45 + i * 0.0001:.6f}" lon="{3 + (i % 2) * 0.0003:.6f}"><ele>{300 + i % 50}</ele></trkpt>'
        for i in range(points)
    )
    path.write_text(f"<gpx><trk><name>Test HIL</name><trkseg>{trk}</trkseg></trk></gpx>")


def run(coro):
    return asyncio.run(coro)


@pytest.fixture
def ble():
    if not adapter():
        pytest.skip("no powered Bluetooth adapter on this PC")


@pytest.fixture
def kept(dut, out_dir):
    """The settings and route before the test, put back after it."""
    dut.reset()
    fs = dut.read_storage(out_dir / "before.bin")
    settings = fs.open("/settings", "r").read() if "settings" in fs.listdir("/") else ""
    route = fs.open("/route", "rb").read() if "route" in fs.listdir("/") else None
    dut.reset()
    yield
    dut.reset()
    for line in settings.splitlines():
        if "=" in line:
            dut.send("set {} {}".format(*line.split("=", 1)))
    if route is not None:
        send_route.send(dut, route)
    else:
        dut.send("route clear")


def test_hello_and_settings(dut, ble, kept):
    async def go():
        app = await App.connect(dut.ble_address)
        try:
            assert (await app.request(HELLO))[0] == DEVICE_INFO
            op, text = await app.request(GET_SETTINGS)
            assert op == SETTINGS and b"scroll_s=" in text, text
            scroll = int(text.decode().split("scroll_s=")[1].split()[0])
            new = 7 if scroll != 7 else 9
            assert (await app.request(SET_SETTINGS, f"scroll_s={new}\n".encode()))[0] == OK
            op, text = await app.request(GET_SETTINGS)
            assert f"scroll_s={new}\n".encode() in text
            # refused: all or nothing, the key named
            op, p = await app.request(SET_SETTINGS, b"scroll_s=5\nbacklight=180\n")
            assert (op, p) == (ERROR, bytes([BAD_SETTING]) + b"backlight")
            op, text = await app.request(GET_SETTINGS)
            assert f"scroll_s={new}\n".encode() in text
            return new
        finally:
            await app.close()

    new = run(go())
    mark = dut.mark()
    dut.send("get scroll_s")
    assert dut.expect(r"usb: scroll_s=(\d+)$", since=mark).group(1) == str(new)


def test_a_route_resumed_after_a_dropped_link(dut, ble, kept):
    with tempfile.TemporaryDirectory() as d:
        gpx = Path(d) / "test.gpx"
        zigzag_gpx(gpx)
        data = send_route.route_bytes(gpx)
    assert len(data) > 10_000, len(data)

    async def go():
        app = await App.connect(dut.ble_address)
        try:
            await app.send_file(ROUTE, data, stop_after=len(data) // 2)
        finally:
            await app.close()
        await asyncio.sleep(2)
        app = await App.connect(dut.ble_address)
        try:
            return await app.send_file(ROUTE, data)
        finally:
            await app.close()

    mark = dut.mark()
    start, sent = run(go())
    assert 0 < start < len(data), f"resumed at {start} of {len(data)}"
    assert sent == len(data) - start, "only the missing part sent"
    dut.expect(r"route: loaded \"Test HIL\"", timeout=10, since=mark)
