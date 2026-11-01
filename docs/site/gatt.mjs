// Musette's virtual central. A board power cycle invalidates the link, MTU, handles and CCCDs together.
const hex4 = (handle) => '0x' + handle.toString(16).padStart(4, '0');
export class GattBridge {
  device = null; connected = false; mtu = 0; powered = true; wanted = false;
  chars = new Map(); subscriptions = new Set(); waiters = []; queue = Promise.resolve();
  generation = 0; connecting = null;
  constructor(send, notify, onError = () => {}) {
    this.send = send; this.notify = notify; this.onError = onError;
  }
  reset(preserve = false, reason = 'compteur redémarré') {
    this.generation++;
    this.device = null; this.connected = false; this.mtu = 0; this.chars.clear(); this.connecting = null;
    if (!preserve) { this.wanted = false; this.subscriptions.clear(); }
    const waiting = this.waiters.splice(0);
    for (const w of waiting) { clearTimeout(w.timer); w.reject(new Error(reason)); }
  }
  setPowered(on) {
    if (on === this.powered) return;
    this.powered = on;
    if (!on) this.reset(true, 'compteur éteint');
  }
  heard(m) {
    if (m.error) {
      const w = this.waiters.shift();
      if (w) { clearTimeout(w.timer); w.reject(new Error(m.error)); }
      return;
    }
    const line = m.line;
    const field = (key) => line.match(new RegExp(`${key}=(0x[0-9a-f]+|[0-9a-f-]+)`))?.[1];
    if (line.startsWith('advertising ')) {
      const names = [...line.matchAll(/name="([^"]*)"/g)].map((x) => x[1]);
      this.device = { name: names.at(-1) || 'CC600', serviceUuid: field('service') };
      // Keep Musette's iframe and notification listeners. Restore the radio link and subscriptions
      // after boot, before processing another request from the app.
      if (this.wanted && !this.connected) {
        const generation = this.generation;
        this.queue = this.queue.then(() => {
          if (generation === this.generation && this.wanted && this.powered) return this.connect();
        }).catch((e) => this.onError(e));
      }
    } else if (line.startsWith('connected ')) this.connected = true;
    else if (line === 'disconnected') this.reset(true, 'liaison Bluetooth interrompue');
    else if (line.startsWith('characteristic ')) this.chars.set(field('uuid'), { value: parseInt(field('handle')), props: parseInt(field('properties')) });
    else if (line.startsWith('notification ') && this.connected) {
      const handle = parseInt(field('handle')), uuid = [...this.chars].find(([, c]) => c.value === handle)?.[0];
      const bytes = (field('value') || '').match(/../g)?.map((b) => parseInt(b, 16)) || [];
      if (uuid) this.notify({ gatt: 'notify', characteristic: uuid, bytes });
    }
    this.waiters = this.waiters.filter((w) => {
      if (!w.match(line)) return true;
      clearTimeout(w.timer); w.resolve(line); return false;
    });
  }
  wait(match, ms = 20000) {
    return new Promise((resolve, reject) => {
      const w = { match, resolve, reject };
      w.timer = setTimeout(() => {
        this.waiters = this.waiters.filter((x) => x !== w);
        reject(new Error('le compteur ne répond pas'));
      }, ms);
      this.waiters.push(w);
    });
  }
  async run(line, match) {
    if (!this.powered) throw new Error('compteur éteint');
    const answer = this.wait(match); this.send({ t: 'ble', line }); return answer;
  }
  char(uuid) {
    const c = this.chars.get(String(uuid).toLowerCase());
    if (!c) throw new Error('caractéristique inconnue : ' + uuid);
    return c;
  }
  async subscribe(uuid) {
    const cccd = this.char(uuid).value + 1;
    await this.run(`subscribe ${cccd}`, (line) => line === `write complete handle=${hex4(cccd)}`);
  }
  connect() {
    if (!this.powered) return Promise.reject(new Error('compteur éteint'));
    if (this.connecting) return this.connecting;
    const generation = this.generation;
    const current = () => { if (generation !== this.generation) throw new Error('compteur redémarré'); };
    const task = (async () => {
      if (!this.device) await this.wait((line) => line.startsWith('advertising '), 60000);
      current();
      if (!this.connected) await this.run('connect', (line) => line.startsWith('connected '));
      current();
      if (!this.mtu) {
        this.mtu = await this.run('mtu 247', (line) => line.startsWith('mtu '))
          .then((line) => parseInt(line.slice(4)), (error) => { current(); if (!this.powered) throw error; return 23; });
      }
      current();
      if (!this.chars.size) {
        await this.run('discover', (line) => line === 'discovery complete');
        current();
        for (const uuid of this.subscriptions) { await this.subscribe(uuid); current(); }
      }
      return { mtu: this.mtu };
    })();
    this.connecting = task;
    task.then(() => { if (this.connecting === task) this.connecting = null; }, () => { if (this.connecting === task) this.connecting = null; });
    return task;
  }
  async request(m) {
    if (m.gatt === 'disconnect') { this.wanted = false; this.subscriptions.clear(); return {}; }
    if (!this.powered) throw new Error('compteur éteint');
    switch (m.gatt) {
      case 'hello':
        if (!this.device) await this.wait((line) => line.startsWith('advertising '), 60000);
        return this.device;
      case 'connect': this.wanted = true; return this.connect();
      case 'subscribe':
        await this.connect(); await this.subscribe(m.characteristic);
        this.subscriptions.add(String(m.characteristic).toLowerCase()); return {};
      case 'write': {
        await this.connect();
        const h = this.char(m.characteristic).value, data = m.bytes.map((b) => b.toString(16).padStart(2, '0')).join('');
        const line = m.withResponse ? `write ${h} ${data}` : `write-cmd ${h} ${data}`;
        const expected = `${m.withResponse ? 'write complete' : 'write command'} handle=${hex4(h)}`;
        await this.run(line, (line) => line === expected); return {};
      }
      default: throw new Error('demande inconnue : ' + m.gatt);
    }
  }
}
