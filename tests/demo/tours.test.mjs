import test from 'node:test';
import assert from 'node:assert/strict';
import { FirmwareTrace, GuidedTour } from '../../docs/site/tours.mjs';

test('USB observations handle split messages and reboot cannot reuse a previous screen', () => {
  const trace = new FirmwareTrace();
  trace.write('\x1b[3'); trace.write('2mINFO screen Ride Alti'); trace.write('tude\x1b[0m\n');
  assert.equal(trace.screen, 'Ride'); assert.equal(trace.detail, 'Altitude');
  trace.reboot();
  assert.equal(trace.screen, undefined);
  trace.write('INFO screen Charging\n');
  assert.equal(trace.screen, 'Charging'); assert.equal(trace.seq, 2);
});

function setup() {
  const trace = new FirmwareTrace();
  let playing = false, restores = 0, actions = 0, rejectPrepare = false;
  const tour = new GuidedTour({
    snapshot: () => ({ ...trace, playing, powered: true }),
    prepare: async () => { if (rejectPrepare) throw new Error('unavailable'); trace.write('screen Home\n'); },
    restore: async () => { restores++; }, present() {},
    perform: async () => { actions++; playing = true; },
  }, () => {});
  return { tour, trace, actions: () => actions, restores: () => restores, reject: () => { rejectPrepare = true; } };
}

test('an action alone cannot unlock a firmware step; repeated pause requires new evidence', async () => {
  const { tour, trace, actions } = setup();
  await tour.start('ride');
  assert.equal(tour.step.id, 'play');
  assert.deepEqual(tour.completed, ['home']);
  tour.update(); assert.equal(tour.step.id, 'play');
  await tour.perform();
  assert.equal(tour.step.id, 'start');
  await tour.perform(); await tour.perform();
  assert.equal(actions(), 2); // one play + one start, no duplicate press while awaiting confirmation
  assert.equal(tour.ready, false);
  trace.write('screen Ride Essential\n'); tour.update();
  assert.equal(tour.ready, false); // screen alone is not proof that recording started
  trace.write('ride 123: started\n'); tour.update();
  assert.equal(tour.step.id, 'page');
  trace.write('screen Ride Altitude\n'); tour.update();
  assert.equal(tour.step.id, 'pause');
  trace.write('ride: paused\nscreen Paused\n'); tour.update();
  assert.equal(tour.step.id, 'resume');
  trace.write('ride: resumed\nscreen Ride Altitude\n'); tour.update();
  assert.equal(tour.step.id, 'pause-again');
  trace.write('screen Paused\n'); tour.update();
  assert.equal(tour.ready, false); // the previous pause's log must not validate this one
  trace.write('ride: paused\n'); tour.update();
  assert.equal(tour.step.id, 'finish');
});

test('short rides save with the actual Enregistrer key and require the FIT save confirmation', async () => {
  const { tour, trace } = setup();
  await tour.start('ride');
  tour.index = tour.tour.steps.findIndex((s) => s.id === 'save');
  trace.write('screen ConfirmEmpty\n'); tour.enter();
  assert.equal(tour.action().target, '.key[data-key="1"]');
  trace.write('screen Saved\n'); tour.update(); assert.equal(tour.ready, false);
  trace.write('ride 123: saved, Totals {}\n'); tour.update(); assert.equal(tour.step.id, 'done');
  tour.index = tour.tour.steps.findIndex((s) => s.id === 'save');
  trace.write('screen ConfirmStop\n'); tour.enter();
  assert.equal(tour.action().target, '.key[data-key="2"]');
});

test('automatic progression waits for both the action and the held key to finish', async () => {
  const { tour, trace } = setup();
  await tour.start('ride'); await tour.perform();
  let releaseAction;
  let keysReleased = false;
  const snapshot = tour.runtime.snapshot;
  tour.runtime.snapshot = () => ({ ...snapshot(), keysReleased });
  tour.runtime.perform = () => new Promise((resolve) => { releaseAction = resolve; });
  const pending = tour.perform();
  trace.write('ride 123: started\nscreen Ride Essential\n'); tour.update();
  assert.equal(tour.step.id, 'start');
  releaseAction(); await pending;
  assert.equal(tour.step.id, 'start');
  keysReleased = true; tour.update();
  assert.equal(tour.step.id, 'page');
});

test('leaving a failed preparation restores once and a second start cannot race it', async () => {
  const { tour, restores, reject } = setup();
  reject(); await tour.start('sensors');
  assert.equal(tour.state, 'error');
  await tour.start('ride'); assert.equal(tour.tour.id, 'sensors');
  await Promise.all([tour.leave(), tour.leave()]);
  assert.equal(tour.state, 'idle'); assert.equal(restores(), 1);
});
