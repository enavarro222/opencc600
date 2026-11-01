import test from 'node:test';
import assert from 'node:assert/strict';
import { GattBridge } from '../../docs/site/gatt.mjs';

const RX = 'c039ed02-4557-4b35-bc20-5c9320871456', TX = 'c039ed03-4557-4b35-bc20-5c9320871456';
const tick = () => new Promise((resolve) => setImmediate(resolve));
function radio() {
  const commands = [], notifications = [];
  const state = { rx: 9, tx: 11, blocked: false };
  const bridge = new GattBridge(({ line }) => {
    commands.push(line);
    if (state.blocked) return;
    queueMicrotask(() => {
      const emit = (line) => bridge.heard({ line });
      if (line === 'connect') emit('connected handle=0x0001 guest=peripheral');
      if (line.startsWith('mtu ')) emit('mtu 247');
      if (line === 'discover') {
        emit(`characteristic uuid=${RX} handle=0x${state.rx.toString(16)} properties=0x08`);
        emit(`characteristic uuid=${TX} handle=0x${state.tx.toString(16)} properties=0x10`);
        emit('discovery complete');
      }
      const handle = Number(line.split(' ')[1]).toString(16).padStart(4, '0');
      if (line.startsWith('subscribe ') || line.startsWith('write ')) emit(`write complete handle=0x${handle}`);
      if (line.startsWith('write-cmd ')) emit(`write command handle=0x${handle}`);
    });
  }, (message) => notifications.push(message));
  const advertise = () => bridge.heard({ line: 'advertising name="OpenCC600" service=c039ed01-4557-4b35-bc20-5c9320871456' });
  return { bridge, commands, notifications, state, advertise };
}
test('power cycle reconnects the same app, rediscovers handles and restores notifications', async () => {
  const { bridge, commands, notifications, state, advertise } = radio();
  advertise();
  assert.equal((await bridge.request({ gatt: 'connect' })).mtu, 247);
  await bridge.request({ gatt: 'subscribe', characteristic: TX });
  bridge.setPowered(false);
  assert.equal(bridge.connected, false); assert.equal(bridge.device, null); assert.equal(bridge.chars.size, 0);
  await assert.rejects(bridge.request({ gatt: 'write', characteristic: RX, bytes: [1] }), /éteint/);
  state.rx = 19; state.tx = 21;
  bridge.setPowered(true); bridge.reset(true); advertise(); await bridge.queue;
  assert.equal(commands.filter((c) => c === 'connect').length, 2);
  assert(commands.includes('subscribe 12')); assert(commands.includes('subscribe 22'));
  await bridge.request({ gatt: 'write', characteristic: RX, bytes: [1, 255], withResponse: false });
  assert.equal(commands.at(-1), 'write-cmd 19 01ff');
  bridge.heard({ line: 'notification handle=0x0015 value=012aff' });
  assert.deepEqual(notifications.at(-1), { gatt: 'notify', characteristic: TX, bytes: [1, 42, 255] });
  assert.equal(bridge.waiters.length, 0);
});
test('a reboot rejects pending operations and cannot turn a canceled MTU exchange into a live connection', async () => {
  const { bridge, state, commands, advertise } = radio();
  advertise(); state.blocked = true;
  const connecting = bridge.request({ gatt: 'connect' });
  bridge.heard({ line: 'connected handle=0x0001 guest=peripheral' });
  await tick(); assert.equal(commands.at(-1), 'mtu 247');
  const failed = assert.rejects(connecting, /redémarré/);
  bridge.reset(true); await failed;
  assert.equal(bridge.connected, false); assert.equal(bridge.mtu, 0); assert.equal(bridge.waiters.length, 0);
});
test('pending writes fail immediately when the device powers off', async () => {
  const { bridge, state, advertise } = radio();
  advertise(); await bridge.request({ gatt: 'connect' });
  state.blocked = true;
  const writing = bridge.request({ gatt: 'write', characteristic: RX, bytes: [1], withResponse: true });
  await tick();
  const failed = assert.rejects(writing, /éteint/);
  bridge.setPowered(false); await failed;
  assert.equal(bridge.waiters.length, 0);
});
test('an app that disconnected is not reconnected automatically after reboot', async () => {
  const { bridge, commands, advertise } = radio();
  advertise(); await bridge.request({ gatt: 'connect' });
  await bridge.request({ gatt: 'subscribe', characteristic: TX });
  await bridge.request({ gatt: 'disconnect' });
  bridge.reset(true); advertise(); await bridge.queue;
  assert.equal(commands.filter((c) => c === 'connect').length, 1);
  assert.equal(bridge.subscriptions.size, 0);
});
