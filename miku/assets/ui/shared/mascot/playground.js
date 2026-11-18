/**
 * mascot/playground.js — Controlador standalone para probar el muñeco aislado (mascot.html)
 * Carga mascot.config.json y permite cambiar estado, atuendo, color y coletas sin Island.
 */
(async function () {
  'use strict';

  const cv = document.getElementById('mk');
  const statesHost = document.getElementById('states');
  const outfitsHost = document.getElementById('outfits');
  const colInput = document.getElementById('col');
  const tailsBtn = document.getElementById('toggleTails');

  let cfg = null;
  try {
    const res = await fetch('/shared/mascot/mascot.config.json');
    if (res.ok) cfg = await res.json();
  } catch (_) {}

  window.MikuMascot.mount(cv, { config: cfg || undefined });
  const resolvedCfg = window.MikuMascot.getConfig();

  function syncActiveButtons() {
    const st = window.MikuMascot.getPhys();
    statesHost.querySelectorAll('button').forEach(b => b.classList.toggle('on', b.dataset.state === st.base));
    outfitsHost.querySelectorAll('button').forEach(b => b.classList.toggle('on', b.dataset.outfit === st.outfit));
    if (tailsBtn) tailsBtn.textContent = `Coletas: ${st.twintails ? 'ON' : 'OFF'}`;
  }

  Object.keys(window.MikuMascot.STATES).forEach(name => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = name;
    b.dataset.state = name;
    b.addEventListener('click', () => {
      window.buddy.setState(name);
      syncActiveButtons();
    });
    statesHost.appendChild(b);
  });

  (resolvedCfg.outfits || []).forEach(o => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = o.labelEs || o.id;
    b.dataset.outfit = o.id;
    b.addEventListener('click', () => {
      window.buddy.setOutfit(o.id);
      window.buddy.emote('happy', 900);
      syncActiveButtons();
    });
    outfitsHost.appendChild(b);
  });

  if (colInput) {
    colInput.addEventListener('input', e => window.buddy.setColor(e.target.value));
  }

  if (tailsBtn) {
    tailsBtn.addEventListener('click', () => {
      const next = !window.MikuMascot.getPhys().twintails;
      window.buddy.setTwintails(next);
      syncActiveButtons();
    });
  }

  window.addEventListener('pointermove', e => {
    const r = cv.getBoundingClientRect();
    if (!r.width) return;
    const nx = Math.max(-1, Math.min(1, (e.clientX - r.left - r.width / 2) / 260));
    const ny = Math.max(-1, Math.min(1, (e.clientY - r.top - r.height / 2) / 180));
    window.MikuMascot.lookAt(nx, ny);
  }, { passive: true });

  window.addEventListener('message', e => {
    const m = e.data || {};
    if (m.k !== 'miku') return;
    if (m.state) window.buddy.setState(m.state);
    if (m.emote) window.buddy.emote(m.emote, m.ms);
    if ('outfit' in m) window.buddy.setOutfit(m.outfit);
    if (m.color) window.buddy.setColor(m.color);
    if ('twintails' in m) window.buddy.setTwintails(m.twintails);
    syncActiveButtons();
  });

  syncActiveButtons();
})();
