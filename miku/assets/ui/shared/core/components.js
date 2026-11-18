/**
 * core/components.js — Controles Físicos de Isla y Gráficos SVG Seguros
 * Incluyen SmoothSlider, Slosh (líquido con inercia), SlideConfirm, mkSound
 * (mini-reproductor expansible) y el asterisco térmico de audio `drawStar`.
 */
(function (global) {
  'use strict';

  const kc = (v, a, b) => Math.min(b, Math.max(a, v));
  const QUART = t => 1 - (1 - t) ** 4;
  const FLAT = t => t;
  const SWING = t => (t < 0.5 ? 4 * t ** 3 : 1 - ((-2 * t + 2) ** 3) / 2);

  function ic(name) {
    return `<svg class="i"><use href="#i-${name}"/></svg>`;
  }

  function mkTw(a, b, ms, ease, fn, done) {
    let id, t0 = performance.now();
    const f = n => {
      const t = Math.min(1, (n - t0) / ms);
      fn(a + (b - a) * ease(t));
      if (t < 1) id = requestAnimationFrame(f);
      else done?.();
    };
    id = requestAnimationFrame(f);
    return () => cancelAnimationFrame(id);
  }

  function mkSp(x, fn) {
    const s = {
      x, v: 0, t: x, raf: 0, k: 300, c: 30, m: 1, cb: null,
      to(t, k, c, m, cb) {
        s.t = t; s.k = k; s.c = c; s.m = m || 1; s.cb = cb || null;
        if (s.raf) return;
        let p = 0;
        const f = n => {
          const dt = p ? Math.min(0.02, (n - p) / 1000) : 0.016;
          p = n;
          s.v += ((-s.k * (s.x - s.t) - s.c * s.v) / s.m) * dt;
          s.x += s.v * dt;
          fn(s.x);
          if (Math.abs(s.x - s.t) < 0.02 && Math.abs(s.v) < 0.02) {
            s.x = s.t; s.v = 0; s.raf = 0; fn(s.x);
            const c2 = s.cb; s.cb = null; c2?.(); return;
          }
          s.raf = requestAnimationFrame(f);
        };
        s.raf = requestAnimationFrame(f);
      },
      stop() { cancelAnimationFrame(s.raf); s.raf = 0; },
      set(v) { s.stop(); s.x = v; s.v = 0; fn(v); }
    };
    return s;
  }

  function SmoothSlider(host, { value = 80, min = 0, max = 100, step = 1, unit = '%', onChange }) {
    if (!host) return null;
    let cur = Math.max(min, Math.min(max, value));
    let dragging = false;
    let activePointerId = null;

    host.innerHTML = `
      <div class="so-slider" tabindex="0" role="slider" aria-valuenow="${cur}" aria-valuemin="${min}" aria-valuemax="${max}">
        <svg class="so-spk so-spk-mute" viewBox="0 0 24 24"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><line x1="23" y1="9" x2="17" y2="15"/><line x1="17" y1="9" x2="23" y2="15"/></svg>
        <div class="so-slider-track"><div class="so-slider-fill" style="width:${((cur - min) / (max - min) * 100).toFixed(1)}%"></div></div>
        <svg class="so-spk so-spk-high" viewBox="0 0 24 24"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07"/></svg>
        <span class="so-slider-val">${Math.round(cur)}${unit}</span>
      </div>`;

    const slider = host.querySelector('.so-slider');
    const track = host.querySelector('.so-slider-track');
    const fill = host.querySelector('.so-slider-fill');
    const valText = host.querySelector('.so-slider-val');
    const spkMute = host.querySelector('.so-spk-mute');
    const spkHigh = host.querySelector('.so-spk-high');

    const update = clientX => {
      const rect = track.getBoundingClientRect();
      if (!rect.width) return;
      const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
      let raw = min + ratio * (max - min);
      if (step > 0) raw = Math.round(raw / step) * step;
      cur = Math.max(min, Math.min(max, raw));
      fill.style.width = ((cur - min) / (max - min) * 100).toFixed(1) + '%';
      valText.textContent = Math.round(cur) + unit;
      slider.setAttribute('aria-valuenow', String(Math.round(cur)));
      onChange?.(cur);
    };

    const onPointerMove = e => { if (dragging) update(e.clientX); };
    const onPointerUp = () => {
      if (!dragging) return;
      dragging = false;
      global.removeEventListener('pointermove', onPointerMove);
      global.removeEventListener('pointerup', onPointerUp);
      global.removeEventListener('pointercancel', onPointerUp);
      if (activePointerId !== null) {
        try { slider.releasePointerCapture(activePointerId); } catch (_) {}
        activePointerId = null;
      }
    };

    slider.addEventListener('pointerdown', e => {
      if (e.button !== 0 || e.target.closest('.so-spk') || e.target.closest('.so-slider-val')) return;
      dragging = true;
      activePointerId = e.pointerId;
      try { slider.setPointerCapture(e.pointerId); } catch (_) {}
      global.addEventListener('pointermove', onPointerMove, { passive: true });
      global.addEventListener('pointerup', onPointerUp);
      global.addEventListener('pointercancel', onPointerUp);
      update(e.clientX);
      global.SoundFX?.click?.();
      e.preventDefault();
    });

    spkMute?.addEventListener('click', () => {
      cur = min; fill.style.width = '0%'; valText.textContent = `${Math.round(cur)}${unit}`;
      global.SoundFX?.pop?.(); onChange?.(cur);
    });
    spkHigh?.addEventListener('click', () => {
      cur = max; fill.style.width = '100%'; valText.textContent = `${Math.round(cur)}${unit}`;
      global.SoundFX?.pop?.(); onChange?.(cur);
    });

    return {
      set(v) {
        cur = Math.max(min, Math.min(max, v));
        fill.style.width = ((cur - min) / (max - min) * 100).toFixed(1) + '%';
        valText.textContent = Math.round(cur) + unit;
        slider.setAttribute('aria-valuenow', String(Math.round(cur)));
      }
    };
  }

  function Slosh(host, o = {}) {
    if (!host) return null;
    host.innerHTML = `<div class="so-track" role="slider" tabindex="0" aria-label="${o.label || 'Volumen'}" aria-valuemin="0" aria-valuemax="100"><div class="so-fill"></div><div class="so-knob"></div></div>`;
    const trEl = host.firstChild, fl = trEl.children[0], kn = trEl.children[1], K = { visc: 15, mom: 55, tilt: 45 };
    const flat = !!global.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    let val = o.value ?? 50, hv = 0, fill = val, fv = 0, held = 0, raf = 0, prev = 0, lastE = Math.round(val), last = { v: val, t: 0 };

    const emit = () => { const r = Math.round(val); if (r !== lastE) { lastE = r; o.onChange?.(r); } };
    const paint = () => {
      fl.style.width = fill.toFixed(2) + '%';
      kn.style.left = val.toFixed(2) + '%';
      fl.style.setProperty('--lean', (flat ? 0 : kc(K.tilt / 100 * fv * 3, -20, 20)).toFixed(2) + 'px');
      trEl.setAttribute('aria-valuenow', String(Math.round(val)));
    };
    const rest = () => !held && Math.abs(hv) < 0.01 && Math.abs(fv) < 0.01 && Math.abs(val - fill) < 0.02;
    const tick = t => {
      const dt = prev ? kc((t - prev) / 16.67, 0, 2.5) : 1; prev = t;
      if (!held && hv) {
        val += hv * dt; hv *= Math.pow(0.86 + K.mom / 100 * 0.115, dt);
        if (val <= 0 || val >= 100) { val = kc(val, 0, 100); hv = 0; }
        if (Math.abs(hv) < 0.01) hv = 0;
      }
      const soft = flat ? 0 : K.visc / 100;
      if (!soft) { fill = val; fv = 0; } else {
        const sf = 0.34 - soft * 0.29, dp = 0.74 + soft * 0.22;
        fv += (val - fill) * sf * dt; fv *= Math.pow(dp, dt); fill += fv * dt;
        if (fill > 100) { fill = 100; fv = -fv * 0.42; } else if (fill < 0) { fill = 0; fv = -fv * 0.42; }
        if (Math.abs(val - fill) < 0.02 && Math.abs(fv) < 0.02) { fill = val; fv = 0; }
      }
      paint(); emit();
      if (rest()) { raf = 0; prev = 0; return; }
      raf = requestAnimationFrame(tick);
    };
    const run = () => { if (!raf) raf = requestAnimationFrame(tick); };
    const at = x => { const b = trEl.getBoundingClientRect(), k = b.width / (trEl.offsetWidth || b.width) || 1; return kc((x - b.left) / k / trEl.offsetWidth * 100, 0, 100); };
    const onMove = e => { if (!held) return; const v = at(e.clientX), dt = Math.max(1, e.timeStamp - last.t); hv = (v - last.v) / dt * 16.67; last = { v, t: e.timeStamp }; val = v; run(); };
    const onUp = () => { if (!held) return; held = 0; trEl.removeAttribute('data-held'); global.removeEventListener('pointermove', onMove); global.removeEventListener('pointerup', onUp); global.removeEventListener('pointercancel', onUp); hv = kc(hv, -6, 6); if (flat) hv = 0; run(); };

    trEl.addEventListener('pointerdown', e => {
      held = 1; trEl.setAttribute('data-held', '1'); hv = 0; val = at(e.clientX); last = { v: val, t: e.timeStamp };
      run(); global.addEventListener('pointermove', onMove); global.addEventListener('pointerup', onUp); global.addEventListener('pointercancel', onUp);
      e.preventDefault();
    });
    paint();
    return { set(v) { val = kc(v, 0, 100); hv = 0; lastE = Math.round(val); run(); } };
  }

  const SV = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">';
  const MEDIA_ICN = {
    back: SV + '<polygon points="19 20 9 12 19 4 19 20"/><line x1="5" x2="5" y1="19" y2="5"/></svg>',
    fwd: SV + '<polygon points="5 4 15 12 5 20 5 4"/><line x1="19" x2="19" y1="5" y2="19"/></svg>',
    heart: SV + '<path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z"/></svg>',
    note: '<svg viewBox="0 0 24 24"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>'
  };

  function mkSound(host, o = {}) {
    if (!host) return null;
    const W = Math.max(220, Math.round(host.clientWidth) || 260), SHUT = 78, PAD = 10, A0 = 40, A1 = 64, LIKE = 30, LEAD = 46, RAIL_H = 3, RAIL_Y = PAD + A1 + 22, OPS_Y = RAIL_Y + RAIL_H + 8 + 10 + 16 + LEAD / 2, OPEN = Math.round(OPS_Y + LEAD / 2 + PAD);
    const mix = (a, b, t) => a + (b - a) * t, reduced = !!global.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    const PL = [6, 4, 10, 4, 10, 20, 6, 20], PY = [6.5, 4, 13.25, 8, 13.25, 16, 6.5, 20], PR = [14, 4, 18, 4, 18, 20, 14, 20], PYR = [13.25, 8, 20, 12, 20, 12, 13.25, 16];
    const quad = (a, b, t) => { let d = ''; for (let i = 0; i < 8; i += 2) d += `${i ? 'L' : 'M'}${mix(a[i], b[i], t).toFixed(2)} ${mix(a[i + 1], b[i + 1], t).toFixed(2)}`; return d + 'Z'; };
    const clock = s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;

    host.innerHTML = `<div class="sn"><div class="sn-box"><span class="sn-art">${MEDIA_ICN.note}</span><span class="sn-say"><span class="sn-t"></span><span class="sn-by"></span></span><span class="sn-bar"><span class="sn-rail"><span class="sn-run"></span></span><span class="sn-clock"><span></span><span></span></span></span><button class="sn-tap" aria-expanded="false" aria-label="Abrir reproductor"></button><button class="sn-like" aria-label="Me gusta" aria-pressed="false" tabindex="-1">${MEDIA_ICN.heart}</button><span class="sn-ops"><button class="sn-op" data-r="prev" aria-label="Anterior">${MEDIA_ICN.back}</button><button class="sn-op" data-lead data-r="toggle" aria-label="Reproducir"><svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"><path/><path/></svg></button><button class="sn-op" data-r="next" aria-label="Siguiente">${MEDIA_ICN.fwd}</button></span></div></div>`;

    const $q = s => host.querySelector(s);
    const sn = $q('.sn'), box = $q('.sn-box'), art = $q('.sn-art'), say = $q('.sn-say'), tt = $q('.sn-t'), by = $q('.sn-by'), bar = $q('.sn-bar'), run = $q('.sn-run'), ck = $q('.sn-clock'), tap = $q('.sn-tap'), like = $q('.sn-like'), ops = $q('.sn-ops'), sv = ops.querySelectorAll('svg'), mp = sv[1].querySelectorAll('path'), lead = ops.children[1];
    let p = 0, u = 0, open = 0, going = !!o.get().playing, at = 0, mt = going ? 0 : 1, c1, c2, c3, liked = 0, TOTAL = 214;

    const layout = () => {
      const zoom = open ? 1 : 1 - 0.035 * Math.sin(Math.PI * kc(u, 0, 1) ** 1.5), h = mix(SHUT, OPEN, p), late = kc((p - 0.6) / 0.4, 0, 1), s = mix(A0, A1, p), sd = mix(24, 34, p), ld = mix(30, LEAD, p);
      sn.style.height = h + 'px'; box.style.height = h + 'px'; box.style.borderRadius = mix(20, 26, p) + 'px'; box.style.scale = String(zoom);
      Object.assign(art.style, { left: PAD + 'px', top: PAD + 'px', width: s + 'px', height: s + 'px', borderRadius: mix(10, 16, p) + 'px' });
      Object.assign(say.style, { left: mix(PAD + A0 + 10, PAD + A1 + 10, p) + 'px', top: PAD + 'px', height: s + 'px', width: mix(94, W - PAD - (PAD + A1 + 10) - (LIKE + 10), p) + 'px' });
      tt.style.fontSize = mix(13, 15.5, p) + 'px'; by.style.fontSize = mix(11, 12, p) + 'px';
      Object.assign(bar.style, { top: mix(SHUT - PAD - RAIL_H, RAIL_Y, p) + 'px', left: PAD + 'px', width: W - PAD * 2 + 'px' });
      ck.style.opacity = String(late);
      Object.assign(tap.style, { left: mix(0, PAD, p) + 'px', top: mix(0, PAD, p) + 'px', width: mix(W, W - PAD * 2, p) + 'px', height: mix(SHUT, A1, p) + 'px', borderRadius: mix(20, 16, p) + 'px' });
      Object.assign(like.style, { left: (W - PAD - LIKE) + 'px', top: PAD + 'px', width: LIKE + 'px', height: LIKE + 'px', opacity: String(late), pointerEvents: late > 0.9 ? 'auto' : 'none' });
      Object.assign(ops.style, { left: mix(W - PAD - 44, W / 2, p) + 'px', top: mix(PAD + A0 / 2, OPS_Y, p) + 'px', gap: mix(5, 14, p) + 'px' });
      ops.children[0].style.width = ops.children[0].style.height = sd + 'px';
      lead.style.width = lead.style.height = ld + 'px';
      ops.children[2].style.width = ops.children[2].style.height = sd + 'px';
      [0, 2].forEach(i => { const z = mix(13, 16, p); sv[i].setAttribute('width', String(z)); sv[i].setAttribute('height', String(z)); });
      sv[1].setAttribute('width', String(mix(14, 18, p))); sv[1].setAttribute('height', String(mix(14, 18, p)));
    };
    const mark = () => {
      const goo = Math.sin(kc(mt, 0, 1) * Math.PI), pull = 1.6 * goo, tip = (going ? -9 : 9) * goo;
      sv[1].style.transform = `rotate(${tip}deg) scale(${1 - 0.13 * goo},${1 + 0.11 * goo})`;
      mp[0].setAttribute('d', quad(PL, PY, mt)); mp[0].setAttribute('transform', `translate(${pull} 0)`);
      mp[1].setAttribute('d', quad(PR, PYR, mt)); mp[1].setAttribute('transform', `translate(${-pull} 0)`);
    };
    const prog = () => { run.style.width = kc(at / TOTAL, 0, 1) * 100 + '%'; ck.children[0].textContent = clock(at); ck.children[1].textContent = '−' + clock(Math.max(0, TOTAL - at)); };
    const sync = () => {
      const m = o.get(), g = !!m.playing;
      tt.textContent = m.playing || (m.title && m.title !== 'Sin reproducción') ? m.title : 'Sin música';
      by.textContent = m.artist || '—';
      if (m.dur) TOTAL = m.dur; if (m.pos != null) at = m.pos; prog();
      if (g !== going) {
        going = g; lead.setAttribute('aria-label', g ? 'Pausa' : 'Reproducir');
        c3?.(); c3 = mkTw(mt, g ? 0 : 1, reduced ? 1 : 300, SWING, v => { mt = v; mark(); });
      }
    };
    const flip = () => {
      open = open ? 0 : 1; tap.setAttribute('aria-expanded', String(!!open)); like.tabIndex = open ? 0 : -1;
      c1?.(); c2?.(); const d = reduced ? 1 : 460;
      const isl = document.getElementById('isl'); isl?.classList.add('live'); setTimeout(() => isl?.classList.remove('live'), d + 80);
      c1 = mkTw(p, open, d, QUART, v => { p = v; layout(); });
      c2 = mkTw(u, open, d, FLAT, v => { u = v; layout(); });
    };

    tap.addEventListener('click', flip);
    like.addEventListener('click', () => { liked = liked ? 0 : 1; like.toggleAttribute('data-on', !!liked); global.Bridge?.like?.(!!liked); });
    ops.addEventListener('click', e => { const b = e.target.closest('[data-r]'); if (!b) return; const r = b.dataset.r; if (r !== 'toggle') at = 0; prog(); o.act?.(r); });
    const iv = setInterval(() => { if (!host.isConnected) { clearInterval(iv); return; } if (going) { at = at >= TOTAL ? 0 : at + 1; prog(); } }, 1000);
    layout(); mark(); prog(); sync();
    return { sync };
  }

  // Asterisco térmico animado cuando hay reproducción activa
  const NEON = [[0, [255, 150, 225]], [0.1, [255, 90, 190]], [0.2, [255, 50, 60]], [0.32, [255, 150, 40]], [0.42, [255, 228, 100]], [0.54, [120, 215, 255]], [0.68, [40, 120, 245]], [0.84, [12, 44, 150]], [1, [6, 18, 80]]];
  const STAR_LUT = (() => {
    const t = new Uint8ClampedArray(1024), F = 34;
    for (let i = 0; i < 256; i++) {
      if (i < F) continue;
      const u = (i - F) / (255 - F);
      let k = 0; while (k < NEON.length - 2 && u > NEON[k + 1][0]) k++;
      const [u0, c0] = NEON[k], [u1, c1] = NEON[k + 1], f = Math.min(1, Math.max(0, (u - u0) / (u1 - u0)));
      for (let q = 0; q < 3; q++) t[i * 4 + q] = c0[q] + (c1[q] - c0[q]) * f;
      t[i * 4 + 3] = 255 * Math.min(1, Math.max(0, (i - F) / 26));
    }
    return t;
  })();

  function drawStar(cv, now) {
    const w = cv.width, h = cv.height, t = now / 1000;
    const f = cv._f || (cv._f = document.createElement('canvas')), k = cv._w || (cv._w = document.createElement('canvas'));
    if (f.width !== w) { f.width = k.width = w; f.height = k.height = h; }
    const g = f.getContext('2d'), off = w * 3;
    g.setTransform(1, 0, 0, 1, 0, 0); g.shadowColor = 'transparent'; g.fillStyle = '#000'; g.fillRect(0, 0, w, h);
    g.shadowColor = '#fff'; g.shadowBlur = w * 0.085; g.shadowOffsetX = off; g.strokeStyle = '#fff'; g.lineCap = 'round'; g.lineWidth = w * 0.1;
    g.save(); g.translate(-off, 0);
    const turn = t * 0.5, L = w * 0.34 * (1 + 0.05 * Math.sin(t * 5));
    for (let r = 0; r < 2; r++) for (let i = 0; i < 3; i++) { const a = turn + (i / 3) * Math.PI, cc = Math.cos(a) * L, ss = Math.sin(a) * L; g.beginPath(); g.moveTo(w / 2 - cc, h / 2 - ss); g.lineTo(w / 2 + cc, h / 2 + ss); g.stroke(); }
    g.restore();
    const q = k.getContext('2d', { willReadFrequently: true });
    q.globalCompositeOperation = 'source-over'; q.fillStyle = '#000'; q.fillRect(0, 0, w, h); q.drawImage(f, 0, 0);
    const hv = now - (cv._hv || -1e9) < 500, hx = hv ? cv._hx * w : w / 2 + Math.cos(t * 1.7) * w * 0.16, hy = hv ? cv._hy * h : h / 2 + Math.sin(t * 1.3) * w * 0.16, rad = w * 0.5, amt = 0.62;
    q.globalCompositeOperation = 'multiply';
    const cg = q.createRadialGradient(hx, hy, 0, hx, hy, rad), val = x => Math.round(255 * (1 - amt * x));
    cg.addColorStop(0, `rgb(${val(1)},${val(1)},${val(1)})`); cg.addColorStop(0.45, `rgb(${val(0.6)},${val(0.6)},${val(0.6)})`); cg.addColorStop(1, 'rgb(255,255,255)');
    q.fillStyle = cg; q.fillRect(0, 0, w, h);
    const src = q.getImageData(0, 0, w, h).data, cx = cv.getContext('2d'), out = cx.createImageData(w, h), d = out.data;
    for (let i = 0; i < src.length; i += 4) { const o = src[i] * 4; d[i] = STAR_LUT[o]; d[i + 1] = STAR_LUT[o + 1]; d[i + 2] = STAR_LUT[o + 2]; d[i + 3] = STAR_LUT[o + 3]; }
    cx.putImageData(out, 0, 0);
  }

  global.MikuComponents = { ic, mkTw, mkSp, SmoothSlider, Slosh, mkSound, drawStar };
})(window);
