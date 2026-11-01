import test from 'node:test';
import assert from 'node:assert/strict';
import { fitCrc, parseFit, recordedValue } from '../../docs/site/track.mjs';
import { Revolutions, SensorSimulation } from '../../docs/site/sensors.mjs';

function fixture({ bigEndian = false, developer = false } = {}) {
  const fields = [[253, 4, 134], [0, 4, 133], [1, 4, 133], [2, 2, 132], [3, 1, 2], [4, 1, 2], [7, 2, 132], [73, 4, 134]];
  const bytes = [developer ? 0x60 : 0x40, 0, bigEndian ? 1 : 0, ...(bigEndian ? [0, 20] : [20, 0]), fields.length, ...fields.flat()];
  if (developer) bytes.push(1, 0, 3, 0);
  function record(timestamp, hr, power, compressed = false, gps = true) {
    bytes.push(compressed ? 0x80 | (timestamp & 31) : 0);
    const data = new Uint8Array(22), v = new DataView(data.buffer); let at = 0;
    const u32 = (n, signed = false) => { v[signed ? 'setInt32' : 'setUint32'](at, n, !bigEndian); at += 4; };
    if (!compressed) u32(timestamp);
    u32(gps ? Math.round(45 * 2 ** 31 / 180) : 0x7fffffff, true);
    u32(gps ? Math.round(6 * 2 ** 31 / 180) : 0x7fffffff, true);
    v.setUint16(at, 3000, !bigEndian); at += 2;
    data[at++] = hr; data[at++] = 85;
    v.setUint16(at, power, !bigEndian); at += 2;
    u32(8333);
    bytes.push(...data.subarray(0, at));
    if (developer) bytes.push(11, 12, 13);
  }
  record(30, 140, 200); record(33, 145, 220, true, false); record(35, 255, 65535, true);
  const out = new Uint8Array(14 + bytes.length + 2), v = new DataView(out.buffer);
  out[0] = 14; out[1] = 0x20; v.setUint32(4, bytes.length, true); out.set([46, 70, 73, 84], 8);
  v.setUint16(12, fitCrc(out.subarray(0, 12)), true); out.set(bytes, 14);
  v.setUint16(out.length - 2, fitCrc(out.subarray(0, -2)), true);
  return out;
}
test('FIT CRC matches the standard CRC-16/ARC check vector', () => {
  assert.equal(fitCrc(new TextEncoder().encode('123456789')), 0xbb3d);
});
for (const bigEndian of [false, true]) for (const developer of [false, true]) {
  test(`FIT: timestamps, scaling, gaps and developer fields (BE=${bigEndian}, dev=${developer})`, () => {
    const track = parseFit(fixture({ bigEndian, developer }).buffer);
    assert.equal(track.pts.length, 2); assert.equal(track.records.length, 3);
    assert.equal(track.pts[0].lat, 45); assert.equal(track.pts[0].ele, 100);
    assert.equal(track.pts[0].power, 200); assert.equal(track.pts[1].hr, null);
    assert.equal(track.pts[1].t - track.pts[0].t, 5000);
    assert.equal(recordedValue(track, [0, 5], 3, 'hr'), 145); // a FIT record with no GPS
    assert.equal(recordedValue(track, [0, 5], 5, 'hr'), null); // explicit missing, no model fallback
    assert.equal(recordedValue(track, [0, 5], 3, 'speed'), 8333 / 1000 * 3.6);
  });
}
test('FIT corruption and truncation fail before loading a route', () => {
  const data = fixture(); data[30] ^= 1;
  assert.throws(() => parseFit(data.buffer), /CRC/);
  assert.throws(() => parseFit(data.slice(0, -3).buffer), /tronqué/);
});
test('GPX records use the trace clock and do not bridge recording gaps', () => {
  const track = { pts: [{ hr: 120 }, { hr: null }, { hr: 130 }] };
  assert.equal(recordedValue(track, [0, 5, 30], 4, 'hr'), 120);
  assert.equal(recordedValue(track, [0, 5, 30], 6, 'hr'), null);
  assert.equal(recordedValue(track, [0, 5, 30], 29, 'hr'), null);
});
const ride = (kmh = 25, grade = 0) => ({ track: { pts: [{ hr: 160 }, { hr: null }] }, secs: [0, 10], at: 0,
  sample: () => ({ kmh, baseKmh: kmh, grade }) });
test('off by default; fixed data, actual radio loss, restore, and unsupported builds', () => {
  const lines = [], states = new Map(), sim = new SensorSimulation((m) => lines.push(m.line), (d, s) => states.set(d.id, s));
  sim.ready = true; sim.tick(0, ride()); assert(lines.every((l) => l.endsWith('off')));
  sim.devices.hr.enabled = true; sim.settings.hr = { source: 'fixed', fixed: 155 };
  sim.tick(1, ride()); assert(lines.at(-3).endsWith('000000000000009b')); assert.equal(states.get('hr').values.hr, 155);
  sim.devices.hr.cut = true; sim.tick(2, ride()); assert(lines.at(-3).endsWith('off')); assert(!states.get('hr').active);
  sim.devices.hr.cut = false; sim.tick(3, ride()); assert(lines.at(-3).endsWith('9b'));
  lines.length = 0; sim.ready = false; sim.tick(4, ride()); assert.equal(lines.length, 0);
});
test('missing trace values and invalid fixed input stop measurements, without replacing them', () => {
  const lines = [], sim = new SensorSimulation((m) => lines.push(m.line));
  sim.ready = true; sim.devices.hr.enabled = true; sim.settings.hr.source = 'trace';
  const r = ride(); r.at = 10; sim.tick(0, r); assert(lines.at(-3).endsWith('idle'));
  sim.settings.hr = { source: 'fixed', fixed: null }; sim.tick(1, r); assert(lines.at(-3).endsWith('idle'));
});
test('CSC pages contain event times and revolution counts; a cut stops both measurements', () => {
  const lines = [], sim = new SensorSimulation((m) => lines.push(m.line));
  sim.ready = true; sim.devices.csc.enabled = true;
  sim.settings.speed = { source: 'fixed', fixed: 2.105 * 3.6 };
  sim.settings.cadence = { source: 'fixed', fixed: 60 };
  sim.tick(0, ride()); sim.tick(1, ride());
  assert(lines.at(-2).endsWith('0004010000040100')); // one wheel turn, one crank turn, 1024 ticks
  sim.devices.csc.cut = true; sim.tick(2, ride()); assert(lines.at(-2).endsWith('off'));
  sim.devices.csc.cut = false; sim.tick(3, ride());
  assert(lines.at(-2).endsWith('000c0200000c0200')); // no turns during the cut
});
test('revolution counters and event clocks wrap at 16 bits', () => {
  const r = new Revolutions(); r.count = 65535;
  assert.deepEqual(r.step(1, 1, 64), [0, 0]);
});
test('model follows effort, grade, stops and gradual cardio recovery', () => {
  const sim = new SensorSimulation(() => {});
  const flat = sim.models({ kmh: 20, grade: 0 }, 60);
  const climbing = sim.models({ kmh: 20, grade: 0.08 }, 60);
  assert(climbing.power > flat.power); assert(climbing.hr > flat.hr);
  const stopped = sim.models({ kmh: 0, grade: 0 }, 1);
  assert.equal(stopped.speed, 0); assert.equal(stopped.cadence, 0); assert.equal(stopped.power, 0);
  assert(stopped.hr < climbing.hr && stopped.hr > 75);
});
