/**
 * settings/particles_header.js — Hero animado 2D con aurora, orbe rotatorio y partículas
 * Pausa automáticamente cuando la pestaña/ventana está oculta, fuera del viewport
 * (IntersectionObserver) o con `prefers-reduced-motion: reduce`.
 */
(function (global) {
  'use strict';

  const SECTION_HUES = Object.freeze({
    general: Object.freeze({ h: 212, s: 75, l: 72 }),
    island: Object.freeze({ h: 212, s: 75, l: 72 }),
    security: Object.freeze({ h: 190, s: 80, l: 62 }),
    permisos: Object.freeze({ h: 190, s: 80, l: 62 }),
    agents: Object.freeze({ h: 34, s: 92, l: 62 }),
    agentes: Object.freeze({ h: 34, s: 92, l: 62 }),
    providers: Object.freeze({ h: 268, s: 75, l: 72 }),
    modelos: Object.freeze({ h: 268, s: 75, l: 72 }),
    keys: Object.freeze({ h: 245, s: 72, l: 70 }),
    mcp: Object.freeze({ h: 198, s: 78, l: 66 }),
    skills: Object.freeze({ h: 198, s: 78, l: 66 }),
    telemetry: Object.freeze({ h: 198, s: 78, l: 66 }),
    notifications: Object.freeze({ h: 28, s: 88, l: 62 }),
    alertas: Object.freeze({ h: 28, s: 88, l: 62 }),
    voice: Object.freeze({ h: 292, s: 74, l: 70 }),
    voz: Object.freeze({ h: 292, s: 74, l: 70 }),
    link: Object.freeze({ h: 150, s: 72, l: 56 }),
    system: Object.freeze({ h: 4, s: 86, l: 62 }),
    peligro: Object.freeze({ h: 4, s: 86, l: 62 })
  });

  function getSectionHue(sectionId) {
    const e = SECTION_HUES[sectionId] || SECTION_HUES.general;
    return { h: e.h, s: e.s, l: e.l };
  }

  function prefersReducedMotion() {
    try {
      return !!global.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    } catch (_) {
      return false;
    }
  }

  const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

  function renderParticlesHeader({
    canvasId = 'heroGalaxyCv',
    sectionId = 'general',
    suiteTag = 'MIKU DESKTOP',
    title = '',
    desc = '',
    statusPillHtml = ''
  } = {}) {
    const hue = getSectionHue(sectionId);
    return `
      <div class="hero miku-hero" style="--h:${hue.h}">
        <canvas class="hero-galaxy-cv" id="${esc(canvasId)}"></canvas>
        <div class="au"><i></i><i></i><i></i></div>
        <svg class="orb" viewBox="0 0 200 200" aria-hidden="true">
          <circle class="a" cx="100" cy="100" r="92"/>
          <circle class="b" cx="100" cy="100" r="70"/>
          <circle class="c" cx="100" cy="100" r="50"/>
          <circle class="k" cx="100" cy="100" r="9"/>
        </svg>
        <div class="miku-hero-head">
          <div class="miku-hero-body" id="ht">
            <small class="miku-hero-tag" id="hg">${esc(suiteTag)}</small>
            <h1 class="miku-hero-title" id="hh">${esc(title)}</h1>
            <p class="miku-hero-desc" id="hp">${esc(desc)}</p>
          </div>
          ${statusPillHtml}
        </div>
      </div>`;
  }

  function createParticlesEngine(cv, state) {
    if (!cv) return null;
    if (cv._mikuParticlesState) {
      if (state?.t) cv._mikuParticlesState.t = { ...state.t };
      cv._mikuParticlesState.kick?.();
      return cv._mikuParticlesState;
    }
    const x = cv.getContext('2d');
    if (!x) return null;

    const st = state || { c: getSectionHue('general'), t: getSectionHue('general') };
    cv._mikuParticlesState = st;

    let W = 0;
    let H = 0;
    let P = [];
    let SS = [];
    const dpr = Math.min(2, global.devicePixelRatio || 1);
    let mx = -999;
    let my = -999;
    let run = false;
    let rafId = 0;
    let ns = 0;
    let inViewport = true;
    const hero = cv.parentNode;

    const updateFlag = () => {
      cv.dataset.particlesRunning = run ? '1' : '0';
    };

    function size() {
      const r = cv.getBoundingClientRect();
      W = Math.max(1, r.width || hero?.clientWidth || 720);
      H = Math.max(1, r.height || hero?.clientHeight || 110);
      cv.width = Math.round(W * dpr);
      cv.height = Math.round(H * dpr);
      x.setTransform(dpr, 0, 0, dpr, 0, 0);
      P = [];
      const n = Math.max(18, Math.min(150, Math.round((W * H) / 3400)));
      for (let i = 0; i < n; i++) {
        const z = Math.random();
        P.push({
          x: Math.random() * W,
          y: Math.random() * H,
          z,
          r: 0.5 + z * 1.6,
          p: Math.random() * 6.283,
          o: (Math.random() - 0.5) * 60,
          vx: 0,
          vy: 0
        });
      }
    }

    function hs(h, l, a) {
      return `hsla(${h.toFixed(0)},${st.c.s.toFixed(0)}%,${l.toFixed(0)}%,${a.toFixed(3)})`;
    }

    function stopEngine() {
      run = false;
      if (rafId) {
        cancelAnimationFrame(rafId);
        rafId = 0;
      }
      updateFlag();
    }

    function frame(tNow) {
      if (!run) {
        rafId = 0;
        updateFlag();
        return;
      }
      const c = st.c;
      const g = st.t;
      const dh = ((g.h - c.h + 540) % 360) - 180;
      c.h = (c.h + dh * 0.06 + 360) % 360;
      c.s += (g.s - c.s) * 0.08;
      c.l += (g.l - c.l) * 0.08;

      if (hero?.style) hero.style.setProperty('--h', c.h.toFixed(1));
      x.clearRect(0, 0, W, H);

      const glowX = mx > 0 ? mx : W * 0.52;
      const glowY = my > 0 ? my : H * 0.42;
      const grad = x.createRadialGradient(glowX, glowY, 4, glowX, glowY, 150);
      grad.addColorStop(0, hs(c.h, 62, 0.14));
      grad.addColorStop(1, 'transparent');
      x.fillStyle = grad;
      x.fillRect(0, 0, W, H);

      for (let i = 0; i < P.length; i++) {
        const q = P[i];
        const sp = 0.16 + q.z * 0.35;
        const angle = q.p + tNow * 0.0004;
        const dx = q.x - mx;
        const dy = q.y - my;
        const distSq = dx * dx + dy * dy;
        if (distSq < 10000 && distSq > 4) {
          const dist = Math.sqrt(distSq);
          const force = (1 - dist / 100) * 0.4;
          q.vx += (dx / dist) * force;
          q.vy += (dy / dist) * force;
        }
        q.vx *= 0.94;
        q.vy *= 0.94;
        q.x += Math.cos(angle) * sp + q.vx;
        q.y += Math.sin(angle) * sp * 0.6 + q.vy;
        if (q.x > W + 4) q.x = -4;
        else if (q.x < -4) q.x = W + 4;
        if (q.y > H + 4) q.y = -4;
        else if (q.y < -4) q.y = H + 4;

        const alpha = (0.4 + 0.5 * Math.sin(tNow * 0.0018 + q.p)) * (0.45 + q.z * 0.55);
        const dotHue = (c.h + q.o * 0.3 + 360) % 360;
        x.fillStyle = hs(dotHue, Math.min(95, c.l + q.z * 16), alpha);
        const dotSize = q.r <= 1.2 ? 1.5 : (q.r <= 1.8 ? 2.2 : 3.0);
        x.beginPath();
        x.arc(q.x, q.y, dotSize, 0, 6.283);
        x.fill();
      }

      if (tNow > ns) {
        ns = tNow + 4800 + Math.random() * 3800;
        SS.push({
          x: Math.random() * W * 0.7,
          y: Math.random() * H * 0.35,
          vx: 5 + Math.random() * 3,
          vy: 1.4 + Math.random() * 1.4,
          l: 1
        });
      }
      for (let k = SS.length - 1; k >= 0; k--) {
        const s = SS[k];
        const sgr = x.createLinearGradient(s.x, s.y, s.x - s.vx * 6, s.y - s.vy * 6);
        sgr.addColorStop(0, hs(c.h, 92, 0.7 * s.l));
        sgr.addColorStop(1, 'transparent');
        x.strokeStyle = sgr;
        x.lineWidth = 1.2;
        x.beginPath();
        x.moveTo(s.x, s.y);
        x.lineTo(s.x - s.vx * 6, s.y - s.vy * 6);
        x.stroke();
        s.x += s.vx;
        s.y += s.vy;
        s.l -= 0.025;
        if (s.l <= 0) SS.splice(k, 1);
      }

      if (prefersReducedMotion() || document.hidden || !inViewport) {
        stopEngine();
        return;
      }
      rafId = requestAnimationFrame(frame);
    }

    st.stop = stopEngine;
    st.isRunning = () => run;
    st.kick = () => {
      if (document.hidden || !inViewport) {
        stopEngine();
        return;
      }
      if (!run) {
        run = true;
        updateFlag();
        rafId = requestAnimationFrame(frame);
      }
    };
    st.setSection = sectionId => {
      st.t = getSectionHue(sectionId);
      if (prefersReducedMotion()) {
        st.c = { ...st.t };
        run = true;
        frame(performance.now());
      } else {
        st.kick();
      }
    };

    if (hero) {
      hero.addEventListener('pointermove', e => {
        const r = cv.getBoundingClientRect();
        mx = e.clientX - r.left;
        my = e.clientY - r.top;
      });
      hero.addEventListener('pointerleave', () => {
        mx = -999;
        my = -999;
      });
    }

    const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(() => size()) : null;
    ro?.observe(cv);
    size();

    const io = typeof IntersectionObserver === 'function'
      ? new IntersectionObserver(entries => {
        inViewport = Boolean(entries[0]?.isIntersecting);
        if (inViewport) st.kick();
        else stopEngine();
      })
      : null;
    io?.observe(cv);

    const onVis = () => {
      if (document.hidden) stopEngine();
      else st.kick();
    };
    document.addEventListener('visibilitychange', onVis);

    st.destroy = () => {
      stopEngine();
      ro?.disconnect();
      io?.disconnect();
      document.removeEventListener('visibilitychange', onVis);
    };

    st.kick();
    return st;
  }

  global.MikuParticlesHeader = {
    SECTION_HUES,
    getSectionHue,
    renderParticlesHeader,
    createParticlesEngine
  };
})(window);
