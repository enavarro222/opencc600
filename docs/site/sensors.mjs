import { recordedValue } from './track.mjs';

export const MEASURES = {
  hr: { label: 'Cardio', unit: 'bpm', min: 1, max: 240, fixed: 140, step: 1 },
  speed: { label: 'Vitesse', unit: 'km/h', min: 0, max: 150, fixed: 25, step: 0.1 },
  cadence: { label: 'Cadence', unit: 'tr/min', min: 0, max: 250, fixed: 85, step: 1 },
  power: { label: 'Puissance', unit: 'W', min: 0, max: 2500, fixed: 200, step: 1 },
};
export const DEVICES = [
  { id: 'hr', name: 'Cardio', type: 120, number: 6001, keys: ['hr'] },
  { id: 'csc', name: 'Vitesse / cadence', type: 121, number: 6002, keys: ['speed', 'cadence'] },
  { id: 'power', name: 'Puissance', type: 11, number: 6003, keys: ['power'] },
];
const clamp = (x, min, max) => Math.max(min, Math.min(max, x));

// Fractional turns stay internal; the wire carries the last whole turn and its actual event time.
export class Revolutions {
  count = 0; time = 0;
  step(rate, dt, now) {
    if (rate > 0 && dt > 0) {
      const before = Math.floor(this.count);
      this.count += rate * dt;
      if (Math.floor(this.count) !== before) this.time = Math.round((now - (this.count % 1) / rate) * 1024) & 65535;
    }
    return [this.time, Math.floor(this.count) & 65535];
  }
}
export class SensorSimulation {
  constructor(send, render = () => {}) {
    this.send = send; this.render = render;
    this.settings = Object.fromEntries(Object.keys(MEASURES).map((key) => [key, { source: 'model', fixed: MEASURES[key].fixed }]));
    this.devices = Object.fromEntries(DEVICES.map((d) => [d.id, { enabled: false, cut: false }]));
    this.ready = false;
    this.reset();
  }
  reset() {
    this.last = null; this.wheel = new Revolutions(); this.crank = new Revolutions();
    this.heart = 75; this.watts = 0; this.event = 0; this.accumulated = 0;
  }
  models(q, dt) {
    // Rider + bicycle: rolling resistance, gravity and aerodynamic drag; no effort while stopped/coasting.
    const v = Math.max(0, q.kmh) / 3.6, slope = clamp(q.grade ?? 0, -0.3, 0.3);
    const target = clamp((85 * 9.81 * (0.005 + slope) + 0.5 * 1.2 * 0.35 * v * v) * v / 0.97, 0, 2500);
    this.watts += (target - this.watts) * (1 - Math.exp(-dt / 3));
    if (!v) this.watts = 0;
    const heartTarget = 75 + 130 * this.watts / (this.watts + 180);
    this.heart += (heartTarget - this.heart) * (1 - Math.exp(-dt / 30));
    return { hr: Math.round(this.heart), speed: clamp(q.kmh, 0, 150),
      cadence: v && target >= 20 ? Math.round(clamp(60 + q.kmh * 0.8 - slope * 60, 45, 110)) : 0,
      power: Math.round(this.watts) };
  }
  tick(now, ride) {
    const dt = this.last === null || now < this.last ? 0 : now - this.last;
    this.last = now;
    const q = ride.track ? ride.sample(ride.at) : { kmh: 0, grade: 0 };
    if (ride.freePosition) q.kmh = 0;
    const model = this.models(q, dt), values = {};
    for (const [key, spec] of Object.entries(MEASURES)) {
      const s = this.settings[key];
      let value = null;
      if (s.source === 'fixed') value = s.fixed;
      else if (s.source === 'model') value = model[key];
      else if (ride.track) {
        value = recordedValue(ride.track, ride.secs, ride.at, key);
        if (key === 'speed' && value === undefined) value = q.baseKmh ?? q.kmh;
      }
      values[key] = Number.isFinite(value) && value >= spec.min && value <= spec.max ? value : null;
    }
    const csc = this.devices.csc;
    const transmitting = this.ready && csc.enabled && !csc.cut && values.speed !== null && values.cadence !== null;
    const wheel = this.wheel.step(transmitting ? values.speed / 3.6 / 2.105 : 0, dt, now);
    const crank = this.crank.step(transmitting ? values.cadence / 60 : 0, dt, now);
    for (const device of DEVICES) {
      const state = this.devices[device.id];
      const missing = device.keys.filter((k) => values[k] === null);
      let payload = 'off', page = new Uint8Array(8), view = new DataView(page.buffer);
      if (state.enabled && !state.cut) {
        payload = 'idle';
        if (!missing.length) {
          if (device.id === 'hr') { page[7] = Math.round(values.hr); }
          if (device.id === 'csc') {
            [crank[0], crank[1], wheel[0], wheel[1]].forEach((n, i) => view.setUint16(i * 2, n, true));
          }
          if (device.id === 'power') {
            page[0] = 0x10; page[1] = this.event++ & 255; page[2] = page[3] = 255; // no duplicated cadence
            this.accumulated = (this.accumulated + Math.round(values.power)) & 65535;
            view.setUint16(4, this.accumulated, true); view.setUint16(6, Math.round(values.power), true);
          }
          payload = [...page].map((b) => b.toString(16).padStart(2, '0')).join('');
        }
      }
      if (this.ready) this.send({ t: 'board_cmd', line: `ant ${device.type} ${device.number} 1 ${payload}` });
      const active = this.ready && state.enabled && !state.cut && !missing.length;
      const status = !this.ready ? 'Émulateur sans simulation de capteurs'
        : !state.enabled ? 'Désactivé' : state.cut ? 'Coupure · aucune transmission'
        : missing.length ? `Indisponible : ${missing.map((k) => MEASURES[k].label.toLowerCase()).join(', ')} · aucune mesure transmise`
        : 'Diffusion ANT+';
      this.render(device, { active, values, status, at: active ? now : null });
    }
  }
}

export function mountSensors(root, simulation, refresh) {
  for (const d of DEVICES) {
    const card = document.createElement('fieldset'); card.className = 'sensor-card';
    card.innerHTML = `<legend>${d.name} <small>ANT+ ${d.number}</small></legend>
      <div class="controls"><label><input id="sensor-${d.id}-enabled" type="checkbox"> Activer le capteur</label>
      <button class="act small" id="sensor-${d.id}-cut" type="button" aria-pressed="false" disabled>Simuler une coupure</button></div>
      ${d.keys.map((key) => { const m = MEASURES[key]; return `<div class="sensor-measure">
        <label for="sensor-${key}-source">${m.label} · source</label>
        <select id="sensor-${key}-source"><option value="fixed">Fixe</option><option value="trace">Trace GPX / FIT</option><option value="model" selected>Modèle</option></select>
        <label class="sensor-fixed" id="sensor-${key}-fixed-label" hidden>Valeur (${m.unit}) <input id="sensor-${key}-fixed" type="number" min="${m.min}" max="${m.max}" step="${m.step}" value="${m.fixed}"></label>
        <output id="sensor-${key}-value" class="sensor-value" aria-label="${m.label} diffusée">—</output>
      </div>`; }).join('')}
      <p class="sensor-status" id="sensor-${d.id}-status"></p><p class="note" id="sensor-${d.id}-time"></p>`;
    root.append(card);
    const enabled = card.querySelector(`#sensor-${d.id}-enabled`), cut = card.querySelector(`#sensor-${d.id}-cut`);
    enabled.onchange = () => { simulation.devices[d.id].enabled = enabled.checked; cut.disabled = !enabled.checked; refresh(); };
    cut.onclick = () => {
      const state = simulation.devices[d.id]; state.cut = !state.cut;
      cut.setAttribute('aria-pressed', String(state.cut)); cut.textContent = state.cut ? 'Rétablir le capteur' : 'Simuler une coupure'; refresh();
    };
    for (const key of d.keys) {
      const source = card.querySelector(`#sensor-${key}-source`), input = card.querySelector(`#sensor-${key}-fixed`);
      source.onchange = () => {
        simulation.settings[key].source = source.value;
        card.querySelector(`#sensor-${key}-fixed-label`).hidden = source.value !== 'fixed'; refresh();
      };
      input.oninput = () => {
        simulation.settings[key].fixed = input.value !== '' && input.validity.valid ? Number(input.value) : null;
        input.setAttribute('aria-invalid', String(!input.validity.valid || input.value === '')); refresh();
      };
    }
  }
  simulation.render = (d, state) => {
    root.querySelector(`#sensor-${d.id}-enabled`).disabled = !simulation.ready;
    root.querySelector(`#sensor-${d.id}-cut`).disabled = !simulation.ready || !simulation.devices[d.id].enabled;
    for (const key of d.keys) {
      const m = MEASURES[key], output = root.querySelector(`#sensor-${key}-value`);
      output.textContent = state.active ? `${state.values[key].toFixed(key === 'speed' ? 1 : 0)} ${m.unit}` : '—';
    }
    root.querySelector(`#sensor-${d.id}-status`).textContent = state.status;
    root.querySelector(`#sensor-${d.id}-time`).textContent = state.at === null ? '' : `Dernier envoi : ${state.at.toFixed(1)} s émulées`;
  };
}
