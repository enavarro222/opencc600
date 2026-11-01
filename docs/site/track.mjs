// Local GPX/FIT decoding for the demo. FIT fields use the profile also written by crates/fit.
const FIT_EPOCH = 631065600000;
const present = (v) => Number.isFinite(v) ? v : null;
export function parseGpx(text) {
  const doc = new DOMParser().parseFromString(text, 'application/xml');
  if (doc.getElementsByTagName('parsererror').length) throw new Error('XML incorrect');
  const elements = (node, tag) => [...node.getElementsByTagNameNS('*', tag)];
  const value = (node, tag) => {
    const text = elements(node, tag)[0]?.textContent?.trim();
    return text ? present(Number(text)) : null;
  };
  const pts = [...elements(doc, 'trkpt'), ...elements(doc, 'rtept')].map((n) => ({
    lat: n.hasAttribute('lat') ? present(Number(n.getAttribute('lat'))) : null,
    lon: n.hasAttribute('lon') ? present(Number(n.getAttribute('lon'))) : null,
    ele: value(n, 'ele') ?? 0, t: present(Date.parse(elements(n, 'time')[0]?.textContent ?? '')),
    hr: value(n, 'hr'), cadence: value(n, 'cad'), power: value(n, 'power') ?? value(n, 'watts'),
    speed: value(n, 'speed') === null ? null : value(n, 'speed') * 3.6,
  })).filter(validPosition);
  if (pts.length < 2) throw new Error('au moins deux points GPS sont nécessaires');
  return { name: elements(doc, 'name')[0]?.textContent?.trim() || 'Trace GPX', pts };
}
function validPosition(p) {
  return p.lat !== null && p.lon !== null && Number.isFinite(p.lat) && Number.isFinite(p.lon)
    && Math.abs(p.lat) <= 90 && Math.abs(p.lon) <= 180;
}
export function fitCrc(bytes) {
  let crc = 0;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = crc & 1 ? (crc >>> 1) ^ 0xa001 : crc >>> 1;
  }
  return crc;
}
export function parseFit(buffer, name = 'Trace FIT') {
  const bytes = new Uint8Array(buffer), v = new DataView(buffer);
  const fail = (s) => { throw new Error(s); };
  if (bytes.length < 14 || bytes[0] < 12 || String.fromCharCode(...bytes.subarray(8, 12)) !== '.FIT') fail('en-tête FIT incorrect');
  const header = bytes[0], end = header + v.getUint32(4, true);
  if (end + 2 !== bytes.length || header > bytes.length - 2) fail('fichier FIT tronqué ou taille incorrecte');
  if (header >= 14 && v.getUint16(header - 2, true) !== 0 && fitCrc(bytes.subarray(0, header)) !== 0) fail('CRC de l’en-tête FIT incorrect');
  if (fitCrc(bytes.subarray(0, end)) !== v.getUint16(end, true)) fail('CRC FIT incorrect');
  let at = header, timestamp = null;
  const definitions = new Map(), records = [];
  const need = (size) => { if (at + size > end) fail('message FIT tronqué'); };
  const scalar = (field, le) => {
    const type = field.type & 31;
    const types = { 0: [1, 'getUint8', 255], 1: [1, 'getInt8', 127], 2: [1, 'getUint8', 255],
      3: [2, 'getInt16', 32767], 4: [2, 'getUint16', 65535], 5: [4, 'getInt32', 2147483647],
      6: [4, 'getUint32', 4294967295], 8: [4, 'getFloat32', NaN], 9: [8, 'getFloat64', NaN],
      10: [1, 'getUint8', 0], 11: [2, 'getUint16', 0], 12: [4, 'getUint32', 0], 13: [1, 'getUint8', 255] };
    const t = types[type];
    if (!t || field.size !== t[0]) return null;
    const n = v[t[1]](at, le);
    return n === t[2] || !Number.isFinite(n) ? null : n;
  };
  while (at < end) {
    const h = bytes[at++], compressed = !!(h & 128), local = compressed ? (h >> 5) & 3 : h & 15;
    if (!compressed && h & 64) {
      need(5);
      const architecture = bytes[at + 1];
      if (architecture > 1) fail('architecture FIT incorrecte');
      const le = architecture === 0, global = v.getUint16(at + 2, le), count = bytes[at + 4]; at += 5;
      need(count * 3);
      const fields = [];
      for (let i = 0; i < count; i++, at += 3) fields.push({ number: bytes[at], size: bytes[at + 1], type: bytes[at + 2] });
      let developerSize = 0;
      if (h & 32) {
        need(1); const count = bytes[at++]; need(count * 3);
        for (let i = 0; i < count; i++, at += 3) developerSize += bytes[at + 1];
      }
      definitions.set(local, { global, le, fields, developerSize });
      continue;
    }
    const def = definitions.get(local);
    if (!def) fail('définition FIT absente');
    const values = new Map();
    if (compressed) {
      if (timestamp === null) fail('horodatage FIT compressé sans référence');
      timestamp += ((h & 31) - (timestamp & 31) + 32) % 32;
      values.set(253, timestamp);
    }
    for (const field of def.fields) {
      if (compressed && field.number === 253) continue;
      need(field.size); values.set(field.number, scalar(field, def.le)); at += field.size;
    }
    need(def.developerSize); at += def.developerSize;
    if (values.get(253) != null) timestamp = values.get(253);
    if (def.global !== 20) continue; // record
    const scaled = (field, scale, offset = 0) => values.get(field) == null ? null : values.get(field) / scale - offset;
    const altitude = scaled(78, 5, 500) ?? scaled(2, 5, 500);
    const speed = scaled(73, 1000) ?? scaled(6, 1000);
    records.push({ lat: scaled(0, 2 ** 31 / 180), lon: scaled(1, 2 ** 31 / 180), ele: altitude,
      t: values.get(253) == null ? null : FIT_EPOCH + values.get(253) * 1000,
      hr: values.get(3) ?? null, cadence: values.get(4) ?? null, power: values.get(7) ?? null,
      speed: speed === null ? null : speed * 3.6 });
  }
  const pts = records.filter(validPosition);
  if (pts.length < 2) fail('au moins deux points GPS sont nécessaires dans le FIT');
  // Missing altitude holds the last known altitude; absent sensor values remain absent.
  let elevation = 0;
  for (const p of pts) { elevation = p.ele ?? elevation; p.ele = elevation; }
  return { name, pts, records };
}

// Hold recorded measurements only until the next record, with a ten-second ceiling for recording gaps.
// Explicit missing values stop immediately; they are never replaced by a model.
export function recordedValue(track, secs, seconds, key) {
  const p = track.pts;
  if (key === 'speed' && !(track.hasRecordedSpeed ?? (track.records || p).some((p) => p.speed != null))) return undefined;
  const timed = track.records && track.timed !== false && Number.isFinite(p[0].t);
  const records = timed ? track.timedRecords ??= track.records.filter((p) => Number.isFinite(p.t)) : p;
  const time = (i) => timed ? (records[i].t - p[0].t) / 1000 : secs[i];
  // The search includes records without GPS, which still carry cardio/power measurements in many FITs.
  const target = timed ? p[0].t + seconds * 1000 : seconds;
  let lo = 0, hi = records.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1, t = timed ? records[mid].t : secs[mid];
    if (t !== null && t <= target) lo = mid + 1; else hi = mid;
  }
  const i = lo - 1;
  if (i < 0 || seconds - time(i) > 10) return null;
  const value = records[i][key];
  return Number.isFinite(value) && value >= 0 ? value : null;
}
