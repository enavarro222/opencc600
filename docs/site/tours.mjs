// These are both the visitor's instructions and the browser tests' scenarios.
// Actions go through the page's real controls; only USB replies validate firmware steps.
const key = (n, long = false) => ({ kind: 'key', target: `.key[data-key="${n}"]`, long });
const click = (target) => ({ kind: 'click', target });
const screen = (name, detail) => (s) => s.screen === name && (!detail || s.detail === detail);
const event = (pattern) => (s, since) => s.events.some((e) => e.seq > since && pattern.test(e.line));
const both = (...checks) => (s, since) => checks.every((check) => check(s, since));
const home = {
  id: 'home', title: 'Le compteur est prêt', target: '#screen',
  text: 'Voici l’accueil du compteur. Les trois touches bleues servent à naviguer ; leur rôle est indiqué à l’écran.',
  check: screen('Home'),
};

export const TOURS = [
  {
    id: 'ride', title: 'Enregistrer une sortie', description: 'Démarrer, changer de page, faire une pause et enregistrer.', duration: '2 min',
    steps: [
      home,
      { id: 'play', title: 'Prendre la route', target: '#play', tab: 'route',
        text: 'Clique sur « Lecture ». Le GPS simulé suit le Tourmalet. La lecture est accélérée cinq fois pour cette visite.',
        action: click('#play'), check: (s) => s.playing },
      { id: 'start', title: 'Démarrer la sortie', action: key(2),
        text: 'Appuie sur la touche en bas à droite : l’enregistrement démarre réellement sur le compteur.',
        check: both(screen('Ride'), event(/ride \d+: started/)) },
      { id: 'page', title: 'Explorer les données', action: key(1),
        text: 'La touche en bas à gauche change de page. Découvre l’altitude, la pente et le dénivelé calculés par le compteur.',
        check: screen('Ride', 'Altitude') },
      { id: 'pause', title: 'Faire une pause', action: key(2),
        text: 'Appuie en bas à droite pour mettre la sortie en pause. Le chronomètre s’arrête, même si le GPS continue à avancer.',
        check: both(screen('Paused'), event(/ride: paused/)) },
      { id: 'resume', title: 'Repartir', action: key(2),
        text: 'La même touche reprend la sortie. Les données précédentes sont conservées.',
        check: both(screen('Ride'), event(/ride: resumed/)) },
      { id: 'pause-again', title: 'Préparer la fin de sortie', action: key(2),
        text: 'Remets la sortie en pause avec la touche en bas à droite pour accéder à « Terminer… ».',
        check: both(screen('Paused'), event(/ride: paused/)) },
      { id: 'finish', title: 'Terminer la sortie', action: key(1),
        text: 'Appuie en bas à gauche sur « Terminer… ». Le compteur demande confirmation avant de fermer la sortie.',
        check: (s) => ['ConfirmStop', 'ConfirmEmpty'].includes(s.screen) },
      { id: 'save', title: 'Enregistrer le fichier',
        text: 'Choisis « Enregistrer ». Pour une sortie très courte, cette action est à gauche ; sinon, elle est à droite. Un vrai fichier FIT est écrit.',
        action: (s) => key(s.screen === 'ConfirmEmpty' ? 1 : 2),
        check: both(screen('Saved'), event(/ride \d+: saved,/)) },
      { id: 'done', title: 'Revenir à l’accueil', action: key(2),
        text: 'Valide avec « OK ». La sortie de cette visite a été enregistrée. Dans tes essais libres, Musette permet de récupérer les fichiers FIT.',
        check: screen('Home') },
    ],
  },
  {
    id: 'sensors', title: 'Connecter une ceinture cardio', description: 'Activer un capteur, le rechercher et afficher ses mesures.', duration: '2 min', feature: 'ant-sensors',
    steps: [
      home,
      { id: 'enable', title: 'Allumer la ceinture simulée', target: '#sensor-hr-enabled', tab: 'sensors',
        text: 'Coche « Activer le capteur » dans la carte Cardio. La ceinture diffuse en ANT+, comme un capteur autour du vélo.',
        action: click('#sensor-hr-enabled'), check: (s) => s.heartEnabled },
      { id: 'menu', title: 'Ouvrir le menu du compteur', action: key(1),
        text: 'Appuie en bas à gauche. « Capteurs » est la première entrée du menu.', check: screen('Menu', 'Sensors') },
      { id: 'sensors', title: 'Ouvrir les capteurs', action: key(2),
        text: 'Valide avec la touche en bas à droite. Le premier rôle proposé est le cardio.', check: screen('Sensors', 'HeartRate') },
      { id: 'search', title: 'Rechercher la ceinture', action: key(2),
        text: 'Choisis « Chercher ». Attends que la ceinture ANT+ 6001 apparaisse ; le compteur la découvre par la radio simulée.',
        check: both(screen('SensorScan', 'HeartRate'), event(/ant: found ant:120:6001:1/)) },
      { id: 'pair', title: 'Appairer le capteur', action: key(2),
        text: 'Choisis la ceinture avec la touche en bas à droite. L’appairage est enregistré par le firmware.',
        check: both(screen('Sensors'), event(/sensors: HeartRate = ant:120:6001:1/), event(/ant: channel 120 .* state 3/)) },
      { id: 'back-menu', title: 'Revenir au menu', action: key(0),
        text: 'La touche en haut à gauche revient à l’écran précédent.', check: screen('Menu') },
      { id: 'back-home', title: 'Revenir à l’accueil', action: key(0),
        text: 'Appuie encore en haut à gauche pour retrouver l’accueil.', check: screen('Home') },
      { id: 'start', title: 'Essayer une sortie avec le cardio', action: key(2),
        text: 'Démarre une sortie. Le compteur ajoute une page Cardio quand une ceinture est connectée.',
        check: both(screen('Ride'), event(/ride \d+: started/)) },
      { id: 'altitude', title: 'Parcourir les pages', action: key(1),
        text: 'Passe à la page Altitude avec la touche en bas à gauche.', check: screen('Ride', 'Altitude') },
      { id: 'cardio', title: 'Retrouver les mesures cardio', action: key(1),
        text: 'Appuie encore sur « Page ». La page Cardio affiche les mesures de la ceinture connectée.', check: screen('Ride', 'Cardio') },
    ],
  },
  {
    id: 'power', title: 'Éteindre et recharger', description: 'Éteindre le compteur, brancher l’USB et le rallumer.', duration: '1 min', feature: 'power-state',
    steps: [
      home,
      { id: 'off', title: 'Éteindre le compteur', action: key(0, true),
        text: 'Depuis l’accueil, maintiens la touche en haut à gauche pour éteindre le compteur.', check: (s) => !s.powered },
      { id: 'charge', title: 'Brancher le câble USB-C', action: click('#usb-plug'),
        text: 'Clique sur la prise sous le compteur, ou glisse-la jusqu’au port. Le compteur se réveille sur son écran de charge.',
        check: both(screen('Charging'), (s) => s.powered && s.usbConnected) },
      { id: 'wake', title: 'Utiliser le compteur pendant la charge', action: key(0, true),
        text: 'Maintiens la touche en haut à gauche. Le compteur quitte la charge et retrouve son accueil.',
        check: both(screen('Home'), event(/charging screen: switched on/)) },
      { id: 'unplug', title: 'Repartir sur batterie', action: click('#usb-plug'),
        text: 'Clique sur la prise pour débrancher le câble. Le compteur reste allumé et fonctionne sur batterie.',
        check: both(screen('Home'), (s) => s.powered && !s.usbConnected) },
    ],
  },
];

/** USB output may arrive split anywhere, including in the middle of an ANSI escape. */
export class FirmwareTrace {
  constructor() { this.reset(); }
  reset() { this.pending = ''; this.seq = 0; this.events = []; this.reboot(); }
  reboot() { this.screen = undefined; this.detail = undefined; }
  write(text) {
    this.pending += text;
    const lines = this.pending.split('\n'); this.pending = lines.pop();
    for (const raw of lines) {
      const line = raw.replace(/\x1b\[[0-9;]*m/g, '').trim();
      this.events.push({ seq: ++this.seq, line });
      const match = line.match(/\bscreen (\w+)(?: (\w+))?/);
      if (match) [, this.screen, this.detail] = match;
    }
    this.events = this.events.slice(-1000);
  }
}

/** The engine never assumes an action succeeded: advance requires observed evidence. */
export class GuidedTour {
  constructor(runtime, render) {
    this.runtime = runtime; this.render = render; this.state = 'idle'; this.busy = false;
  }
  get step() { return this.tour?.steps[this.index]; }
  action() { return typeof this.step?.action === 'function' ? this.step.action(this.runtime.snapshot()) : this.step?.action; }
  async start(id) {
    if (this.state !== 'idle') return;
    const tour = TOURS.find((t) => t.id === id);
    if (!tour) return;
    this.tour = tour; this.index = 0; this.completed = []; this.state = 'preparing'; this.error = ''; this.ready = false; this.render(this);
    try {
      await this.runtime.prepare(tour);
      this.state = 'active'; this.enter();
    } catch (e) { this.fail(e); }
  }
  enter() {
    this.ready = false; this.busy = false; this.performing = false; this.since = this.runtime.snapshot().seq;
    this.deadline = this.action() ? undefined : Date.now() + 90000;
    this.error = ''; this.runtime.present(this.step, this.action()); this.update();
  }
  update() {
    if (this.state !== 'active') return;
    const snapshot = this.runtime.snapshot();
    if (this.step.check(snapshot, this.since)) { this.ready = true; this.deadline = undefined; }
    // A held key must be released before the next instruction uses that same key.
    if (this.ready && !this.performing && snapshot.keysReleased !== false) return this.advance();
    if (this.deadline && Date.now() > this.deadline) return this.fail(new Error('Le compteur n’a pas confirmé cette action. Tu peux recommencer la visite ou revenir aux essais libres.'));
    this.render(this);
  }
  async perform() {
    if (this.state !== 'active' || this.ready || this.busy || !this.action()) return;
    const index = this.index;
    this.busy = true; this.performing = true; this.deadline = Date.now() + 60000; this.render(this);
    try {
      await this.runtime.perform(this.action());
      if (this.state === 'active' && this.index === index) { this.performing = false; this.update(); }
    } catch (e) { if (this.state === 'active' && this.index === index) this.fail(e); }
    // Keep the action disabled until confirmed: a second press could undo a pause or a connection.
  }
  advance() {
    if (this.state !== 'active' || !this.ready) return;
    this.completed.push(this.step.id);
    if (++this.index === this.tour.steps.length) { this.state = 'complete'; this.render(this); }
    else this.enter();
  }
  fail(error) { this.state = 'error'; this.error = error instanceof Error ? error.message : String(error); this.busy = false; this.render(this); }
  async leave() {
    if (['idle', 'preparing', 'leaving'].includes(this.state)) return;
    this.state = 'leaving'; this.render(this);
    try { await this.runtime.restore(); this.state = 'idle'; this.tour = undefined; this.render(this); }
    catch (e) { this.fail(e); }
  }
}

export function mountTours(root, runtime) {
  const icons = [
    '<path d="M6 5h12v18H6zM10 9h4m-4 4h4m-2 4v3m-2-1 2 2 2-2"/>',
    '<path d="M3 14h5l3-8 5 16 3-8h6"/>',
    '<path d="M12 3v9m-5-7a10 10 0 1 0 10 0M21 18v7m-3-3h6"/>',
  ];
  root.innerHTML = `<div id="tour-choices"><h2>On te montre ?</h2>
    <p>Quelques balades guidées pour prendre le compteur en main.</p>
    <div class="tour-cards">${TOURS.map((t, i) => `<button type="button" class="tour-card" data-tour="${t.id}">
      <svg class="tour-card-icon" viewBox="0 0 28 28" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[i]}</svg>
      <strong>${t.title}</strong><span>${t.description}</span><small>Environ ${t.duration}</small></button>`).join('')}</div>
    <p class="note">Tes essais libres sont conservés. Les sorties des visites sont temporaires.</p></div>
    <div id="tour-guide" hidden aria-labelledby="tour-title">
      <div class="tour-heading"><span id="tour-progress"></span><button class="act small" id="tour-quit" type="button">Quitter la visite</button></div>
      <progress id="tour-bar" aria-label="Progression de la visite"></progress>
      <h2 id="tour-title" tabindex="-1"></h2><p id="tour-instruction"></p>
      <p id="tour-feedback" role="status" aria-live="polite"></p>
      <div class="tour-actions"><button class="act" id="tour-perform" type="button">Faire cette étape pour moi</button>
      <button class="act tour-primary" id="tour-finish" type="button" hidden>Retour aux essais libres</button>
      <button class="act" id="tour-retry" type="button" hidden>Recommencer la visite</button></div>
    </div>`;
  const $ = (id) => root.querySelector(`#${id}`);
  let shown, highlighted, rendered;
  const tools = document.getElementById('tour-tools');
  let portal, pointing = false, arrowFrame;
  const pointer = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  pointer.id = 'tour-pointer'; pointer.setAttribute('aria-hidden', 'true'); pointer.toggleAttribute('hidden', true);
  pointer.innerHTML = '<defs><marker id="tour-arrowhead" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M1 1 9 5 1 9Z"/></marker></defs><path id="tour-arrow-line" marker-end="url(#tour-arrowhead)"/>';
  document.body.append(pointer);
  function releaseTools() {
    if (!portal) return;
    portal.node.querySelectorAll('.tour-tool-active').forEach((card) => card.classList.remove('tour-tool-active'));
    portal.marker.replaceWith(portal.node); portal = undefined; tools.hidden = true;
  }
  function drawArrow() {
    arrowFrame = undefined;
    if (!pointing || !highlighted?.isConnected || root.dataset.state !== 'active') { pointer.toggleAttribute('hidden', true); return; }
    const target = highlighted.getBoundingClientRect(), guide = $('tour-guide').getBoundingClientRect();
    if (target.bottom < 0 || target.top > innerHeight || !target.width || !target.height) { pointer.toggleAttribute('hidden', true); return; }
    const ey = target.top + target.height / 2;
    let d, ex;
    if (highlighted.closest('#tour-tools')) {
      // Enter from the empty margin, leaving the control's label and the card title unobstructed.
      const sx = Math.max(20, tools.getBoundingClientRect().left - 16);
      ex = target.left - 10;
      d = `M ${sx} ${Math.max(20, ey - 54)} C ${sx} ${ey - 8}, ${ex - 26} ${ey}, ${ex} ${ey}`;
    } else if (innerWidth > 980 && highlighted.closest('.device') && !highlighted.classList.contains('br')) {
      const sx = guide.right + 12, sy = Math.max(guide.top + 55, Math.min(ey, guide.bottom - 28));
      ex = target.left - 12;
      d = `M ${sx} ${sy} C ${sx + 50} ${sy}, ${sx + 36} ${ey}, ${ex} ${ey}`;
    } else {
      // Short arrows on the far side and on phones avoid crossing the counter's display.
      const right = target.left + target.width / 2 >= innerWidth / 2;
      ex = right ? target.right + 10 : target.left - 10;
      const sx = Math.max(20, Math.min(innerWidth - 20, ex + (right ? 65 : -65)));
      const sy = Math.max(20, ey - 72);
      d = `M ${sx} ${sy} C ${sx} ${ey - 12}, ${ex + (right ? 35 : -35)} ${ey}, ${ex} ${ey}`;
    }
    pointer.toggleAttribute('hidden', false); pointer.setAttribute('viewBox', `0 0 ${innerWidth} ${innerHeight}`);
    pointer.dataset.target = highlighted.id ? `#${highlighted.id}` : `.key[data-key="${highlighted.dataset.key}"]`;
    pointer.dataset.x = ex; pointer.dataset.y = ey;
    pointer.querySelector('#tour-arrow-line').setAttribute('d', d);
  }
  const queueArrow = () => { if (!arrowFrame) arrowFrame = requestAnimationFrame(drawArrow); };
  const resize = new ResizeObserver(queueArrow);
  resize.observe(root); resize.observe(document.getElementById('device')); resize.observe(tools);
  addEventListener('resize', queueArrow);
  addEventListener('scroll', queueArrow, { passive: true });
  document.fonts.ready.then(queueArrow);
  const clearHighlight = () => {
    if (highlighted) { highlighted.classList.remove('tour-target'); resize.unobserve(highlighted); }
    highlighted = undefined; pointing = false; pointer.toggleAttribute('hidden', true);
  };
  const wrapped = {
    ...runtime,
    async prepare(visit) { releaseTools(); await runtime.prepare(visit); },
    async restore() { releaseTools(); await runtime.restore(); },
    present(step, action) {
      releaseTools();
      runtime.present(step, action);
      clearHighlight();
      highlighted = document.querySelector(step.target || action?.target || '#screen');
      if (step.tab && highlighted) {
        // Move the existing controls, keeping their handlers and sensor rendering intact.
        const node = step.tab === 'sensors' ? document.getElementById('sensor-cards') : highlighted;
        const marker = document.createComment('guided control'); node.before(marker);
        portal = { node, marker }; tools.querySelector('#tour-tools-content').append(node);
        highlighted.closest('.sensor-card')?.classList.add('tour-tool-active');
        document.getElementById('tour-tools-title').textContent = step.tab === 'sensors' ? 'La ceinture cardio' : 'Le parcours du Tourmalet';
        tools.hidden = false;
      }
      highlighted?.classList.add('tour-target'); if (highlighted) resize.observe(highlighted);
      pointing = !!action;
      const behavior = matchMedia('(prefers-reduced-motion: reduce)').matches ? 'instant' : 'smooth';
      requestAnimationFrame(() => {
        const mobile = innerWidth <= 980;
        const context = mobile ? root : document.querySelector('main');
        // The stage stays put between physical keys. Environmental controls stay beside it on desktop.
        if (context) {
          const rect = context.getBoundingClientRect();
          const offscreen = rect.top < 0 || (highlighted && highlighted.getBoundingClientRect().bottom > innerHeight - 16)
            || document.getElementById('screen').getBoundingClientRect().bottom > innerHeight - 16;
          if (offscreen) window.scrollTo({ top: Math.max(0, scrollY + rect.top - (mobile ? 12 : 24)), behavior });
        }
        queueArrow();
      });
    },
  };
  const tour = new GuidedTour(wrapped, (t) => {
    // USB is polled often. Unchanged text must not repeatedly trigger the live region.
    const signature = JSON.stringify([t.state, t.tour?.id, t.index, t.ready, t.busy, t.error]);
    if (signature === rendered) return;
    rendered = signature;
    const idle = t.state === 'idle', active = t.state === 'active', complete = t.state === 'complete', error = t.state === 'error';
    root.dataset.state = t.state; root.dataset.tour = t.tour?.id || ''; root.dataset.step = t.step?.id || '';
    root.dataset.completed = JSON.stringify(t.completed || []);
    $('tour-choices').hidden = !idle; $('tour-guide').hidden = idle;
    root.classList.toggle('tour-running', !idle);
    if (!active) { clearHighlight(); releaseTools(); }
    if (idle) { shown = undefined; return; }
    const title = complete ? 'Visite terminée !' : error ? 'La visite est interrompue' : t.state === 'preparing' ? 'Préparation de la visite…' : t.state === 'leaving' ? 'Retour à tes essais libres…' : t.step.title;
    $('tour-progress').textContent = `${t.tour.title} · ${Math.min(t.index + 1, t.tour.steps.length)} / ${t.tour.steps.length}`;
    $('tour-bar').max = t.tour.steps.length; $('tour-bar').value = complete ? t.tour.steps.length : t.index;
    $('tour-title').textContent = title;
    $('tour-instruction').textContent = complete ? 'Tu as essayé ces fonctions sur le vrai firmware du compteur. Tu peux retrouver tes essais libres ou choisir une autre visite.' : error ? t.error : active ? t.step.text : 'Tes essais libres sont conservés pendant cette visite.';
    $('tour-feedback').textContent = active ? t.ready ? 'Action reconnue · relâche la touche pour continuer.' : t.busy ? 'Action effectuée · en attente de la réponse du compteur…' : 'À toi de jouer. La visite avance automatiquement après ton action.' : '';
    $('tour-perform').hidden = !active || !t.action() || t.ready; $('tour-perform').disabled = t.busy;
    $('tour-finish').hidden = !complete;
    $('tour-retry').hidden = !active && !error;
    $('tour-retry').disabled = t.busy && !t.ready;
    $('tour-quit').disabled = ['preparing', 'leaving'].includes(t.state);
    if (shown !== title) { shown = title; $('tour-title').focus({ preventScroll: true }); }
    queueArrow();
  });
  for (const button of root.querySelectorAll('[data-tour]')) button.onclick = () => void tour.start(button.dataset.tour);
  $('tour-perform').onclick = () => void tour.perform();
  $('tour-finish').onclick = () => void tour.leave();
  $('tour-quit').onclick = () => void tour.leave();
  $('tour-retry').onclick = async () => { const id = tour.tour.id; await tour.leave(); if (tour.state === 'idle') void tour.start(id); };
  tour.render(tour);
  return tour;
}
