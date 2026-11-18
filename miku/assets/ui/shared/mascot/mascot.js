/**
 * mascot/mascot.js — Motor Procedural 2D Completamente Desacoplado de Miku
 * ---------------------------------------------------------------------------
 * Este módulo NO conoce ventanas, puertos, WebSocket, rutas backend ni escenas
 * de la isla. Puedes reemplazar este archivo (manteniendo `window.MikuMascot`
 * y `window.buddy`) por sprites PNG/Lottie, SVG o un muñeco 3D en Three.js sin
 * tocar el resto de la aplicación.
 */
(function (global) {
  'use strict';

  const DEFAULT_CONFIG = {
    version: 1,
    name: 'Miku Canvas 2D Procedural',
    defaultColor: '#ffffff',
    accentRgb: [59, 158, 255],
    eyeAccentHex: '#5aaeff',
    blushColor: '#FF9DB8',
    twintailsColors: ['#f4f6fb', '#cbd5e1'],
    enableTwintails: false,
    enableEyeTracking: true,
    enableIdleSleep: true,
    sleepTimeoutMs: 35000,
    scaleMultiplier: 1.0,
    defaultOutfit: 'none',
    presets: ['#F2A765', '#F06CF0', '#6C63FF', '#FF5C66', '#4FD3B1', '#3B9EFF', '#FFD84D', '#9BE15D'],
    outfits: [
      { id: 'none', labelEs: 'Ninguno', labelEn: 'None' },
      { id: 'beanie', labelEs: 'Gorro', labelEn: 'Beanie' },
      { id: 'crown', labelEs: 'Corona', labelEn: 'Crown' },
      { id: 'party', labelEs: 'Fiesta', labelEn: 'Party' },
      { id: 'santa', labelEs: 'Santa', labelEn: 'Santa' },
      { id: 'witch', labelEs: 'Bruja', labelEn: 'Witch' },
      { id: 'shades', labelEs: 'Gafas sol', labelEn: 'Shades' },
      { id: 'round', labelEs: 'Gafas', labelEn: 'Round Glasses' },
      { id: 'bow', labelEs: 'Moño', labelEn: 'Bow' },
      { id: 'scarf', labelEs: 'Bufanda', labelEn: 'Scarf' },
      { id: 'pumpkin', labelEs: 'Calabaza', labelEn: 'Pumpkin' },
      { id: 'bunny', labelEs: 'Orejas', labelEn: 'Bunny Ears' },
      { id: 'halo', labelEs: 'Aureola', labelEn: 'Halo' },
      { id: 'propeller', labelEs: 'Hélice', labelEn: 'Propeller' },
      { id: 'phones', labelEs: 'Audífonos', labelEn: 'Headphones' }
    ]
  };

  const X = {
    idle: { e: 'o', m: 'dot' },
    listening: { e: 'o', m: 'O', fx: 'listen', gi: 1.5 },
    thinking: { arms: [0.5, 0, 1.1, -12], e: 'o', m: 'dot', lk: [0.75, -0.85], fx: 'think', g: [170, 140, 255], gi: 1.3, t: -0.05 },
    talking: { e: 'o', m: 'talk', fx: 'talk', bounce: 1 },
    searching: { e: 'o', m: 'dot', scan: 1, fx: 'search' },
    working: { e: 'o', m: 'frown', br: 0.6, fx: 'work', lk: [0, 0.35] },
    happy: { arms: [0.9, -8, 0.9, -8], e: 'arc', m: 'smile', fx: 'happy', blush: 1, hop: 1 },
    proud: { arms: [0.9, -8, 0.9, -8], e: 'o', m: 'smile', fx: 'proud', sy: 1.05, gi: 1.4 },
    wink: { e: 'wink', m: 'smile', fx: 'happy', t: 0.1 },
    love: { arms: [0.8, -4, 0.8, -4], e: 'o', m: 'smile', fx: 'love', blush: 1, g: [255, 157, 184], gi: 1.3 },
    surprised: { arms: [1.05, -12, 1.05, -12], e: 'o', m: 'O', fx: 'surp', big: 1, jump: 1, trk: 0.25 },
    question: { e: 'o', m: 'dot', fx: 'q', t: 0.12 },
    annoyed: { e: 'ang', m: 'grr', br: 1, fx: 'ann', lk: [0, 0.08], t: -0.03, sy: 0.96, trem: 1, blush: 1, blc: '#ff5a4d', g: [255, 95, 85], gi: 1.4, arms: [0.15, 6, 0.15, 6] },
    sleeping: { e: 'line', m: 'none', fx: 'zzz', sy: 0.92, gi: 0.5, lk: [0, 0.32], trk: 0 },
    error: { lk: [0, 0.5], arms: [0.15, 5, 0.15, 5], e: 'x', m: 'frown', fx: 'err', shake: 1, g: [255, 122, 112] },
    dizzy: { lk: [0, 0], flap: 1, e: 'spiral', m: 'frown', fx: 'diz', wob: 1 },
    look: { e: 'o', m: 'dot' },
    squash: { e: 'arc', m: 'dot', squash: 1 },
    swallow: { e: 'o', m: 'eat', mo: 0.62, big: 1, arms: [0.95, -10, 0.95, -10], gi: 1.45 },
    eat: { e: 'o', m: 'eat', big: 1, arms: [1, -12, 1, -12], gi: 1.6, trk: 0.2 },
    chew: { e: 'arc', m: 'eat', blush: 1, arms: [0.9, -8, 0.9, -8], gi: 1.3, hop: 1 },
    point_upd: { arms: [0.3, 2, 1.15, -24], e: 'wink', m: 'smile', lk: [1.0, -0.15], fx: 'happy', hop: 1, blush: 1, t: 0.08 },
    wave: { e: 'o', m: 'smile', fx: 'wave', wave: 1 }
  };

  let cfg = { ...DEFAULT_CONFIG };
  let cv = null;
  let c = null;
  let BL = [...cfg.accentRgb];
  let EP = [[0, 0, 1, 1], [0, 0, 1, 1]];
  let OUT = cfg.defaultOutfit || 'none';
  let TAILS = cfg.enableTwintails === true;
  let BODY = cfg.defaultColor || '#ffffff';
  let bl = cfg.eyeAccentHex || '#5aaeff';
  const dark = '#172B40';
  const ML = { x: 0, y: -40 };
  const P = { al: 0.5, ll: 0, ar: 0.5, lr: 0, lx: 0, ly: 0, tilt: 0, gi: 1, r: 59, g: 158, b: 255, sq: 0, sv: 0, jy: 0, jv: 0, sy: 1, big: 1, br: 0, bl: 0, mo: 0 };
  const EAT = { on: 0, gx: 0, gy: 0, mo: 0 };
  const TL = [{ a: 0, v: 0 }, { a: 0, v: 0 }];
  const LOCK = { until: 0, prio: 0 };
  const HARD = { error: 3, dizzy: 3, surprised: 2, love: 2, wave: 2, proud: 2, happy: 2 };

  let base = 'idle';
  let cur = 'idle';
  let mx = 0;
  let my = 0;
  let ov = [0, 0];
  let t = 0;
  let ct = 0;
  let last = performance.now();
  let nb = 2;
  let bt = -9;
  let emT = null;
  let OPV = 1;
  let frameRaf = null;
  let paused = false;
  let hidden = false;
  let lastPaint = 0;
  let lastAct = performance.now();
  let isResting = false;
  let restBlinkTimer = null;
  let hooks = {};

  function clamp(v, a = -1, b = 1) { return Math.max(a, Math.min(b, v)); }
  function safeStorageGet(k, fb) { try { return localStorage.getItem(k) ?? fb; } catch (_) { return fb; } }
  function safeStorageSet(k, v) { try { localStorage.setItem(k, v); } catch (_) {} }

  function applyConfig(nextCfg) {
    if (!nextCfg || typeof nextCfg !== 'object') return cfg;
    const normalized = { ...nextCfg };
    const isUntouchedLegacyPhase6 =
      !normalized.schemaVersion &&
      normalized.enableTwintails === true &&
      Array.isArray(normalized.twintailsColors) &&
      String(normalized.twintailsColors[0] || '').toLowerCase() === '#62e8de';
    if (isUntouchedLegacyPhase6) {
      normalized.enableTwintails = false;
      normalized.twintailsColors = [...DEFAULT_CONFIG.twintailsColors];
    }
    cfg = { ...DEFAULT_CONFIG, ...normalized };
    if (Array.isArray(cfg.accentRgb) && cfg.accentRgb.length === 3) BL = [...cfg.accentRgb];
    if (cfg.eyeAccentHex) bl = cfg.eyeAccentHex;
    if (cfg.defaultColor && BODY === '#ffffff') BODY = cfg.defaultColor;
    const schemaVer = safeStorageGet('miku_mascot_schema', null);
    if (schemaVer !== '2') {
      TAILS = cfg.enableTwintails === true;
      safeStorageSet('miku_mascot_schema', '2');
      safeStorageSet('miku_tails', TAILS ? '1' : '0');
    } else if (typeof cfg.enableTwintails === 'boolean') {
      const saved = safeStorageGet('miku_tails', null);
      TAILS = saved === null ? cfg.enableTwintails : saved !== '0';
    }
    const savedOut = safeStorageGet('miku_outfit', null);
    if (savedOut) OUT = savedOut;
    else if (cfg.defaultOutfit) OUT = cfg.defaultOutfit;
    return cfg;
  }

  // Primitivas de dibujo sobre el contexto actual `c`
  const L = (a, b, x, y) => { c.moveTo(a, b); c.lineTo(x, y); };
  const gr = (x0, y0, x1, y1, a, b) => { const g = c.createLinearGradient(x0, y0, x1, y1); g.addColorStop(0, a); g.addColorStop(1, b); return g; };
  function spark(x, y, r) { c.beginPath(); c.moveTo(x, y - r); c.quadraticCurveTo(x, y, x + r, y); c.quadraticCurveTo(x, y, x, y + r); c.quadraticCurveTo(x, y, x - r, y); c.quadraticCurveTo(x, y, x, y - r); c.fill(); }
  function heart(x, y, s) { c.beginPath(); c.moveTo(x, y + s * 0.8); c.bezierCurveTo(x - s * 1.2, y - s * 0.1, x - s * 0.6, y - s * 0.9, x, y - s * 0.35); c.bezierCurveTo(x + s * 0.6, y - s * 0.9, x + s * 1.2, y - s * 0.1, x, y + s * 0.8); c.fill(); }
  function star5(x, y, ro, ri, rot = 0) { c.beginPath(); for (let i = 0; i < 10; i++) { const r = i % 2 ? ri : ro, a = rot - Math.PI / 2 + i * Math.PI / 5; c[i ? 'lineTo' : 'moveTo'](x + Math.cos(a) * r, y + Math.sin(a) * r); } c.closePath(); }
  function gear(x, y, R, n, rot) {
    c.save(); c.translate(x, y); c.rotate(rot);
    const st = 6.2832 / n; c.beginPath(); let f = 1;
    for (let i = 0; i < n; i++) for (const [r, o] of [[0.76, -0.3], [1, -0.17], [1, 0.17], [0.76, 0.3]]) {
      const a = (i + o) * st; c[f ? 'moveTo' : 'lineTo'](Math.cos(a) * R * r, Math.sin(a) * R * r); f = 0;
    }
    c.closePath(); c.moveTo(R * 0.3, 0); c.arc(0, 0, R * 0.3, 0, 6.2832);
    c.fillStyle = gr(-R, -R, R, R, '#a8d6ff', '#3a86ea'); c.fill('evenodd');
    c.strokeStyle = 'rgba(255,255,255,.65)'; c.lineWidth = 1.3; c.beginPath(); c.arc(0, 0, R * 0.62, 3.6, 4.8); c.stroke();
    c.restore();
  }
  function heart2(x, y, sz, rot) {
    c.save(); c.translate(x, y); c.rotate(rot);
    c.shadowColor = 'rgba(255,92,138,.6)'; c.shadowBlur = 8;
    c.fillStyle = gr(0, -sz, 0, sz, '#ffc3d4', '#ff5c8a'); heart(0, 0, sz);
    c.shadowBlur = 0; c.fillStyle = 'rgba(255,255,255,.7)';
    c.beginPath(); c.ellipse(-sz * 0.45, -sz * 0.35, sz * 0.22, sz * 0.13, -0.7, 0, 7); c.fill();
    c.restore();
  }

  const FX = {
    listen() { c.lineCap = 'round'; c.shadowColor = bl; c.shadowBlur = 8; for (let i = 0; i < 3; i++) for (const d of [-1, 1]) { c.globalAlpha = 0.25 + 0.75 * Math.abs(Math.sin(t * 3.6 - i * 0.8)); c.strokeStyle = bl; c.lineWidth = 3.2 - i * 0.6; c.beginPath(); const a = d > 0 ? -0.8 : Math.PI - 0.8; c.arc(80 + d * (53 + i * 9), 66, 10 + i * 5, a, a + 1.6); c.stroke(); } },
    think() {
      c.save(); c.translate(0, Math.sin(t * 2) * 1.5); c.shadowColor = 'rgba(120,180,255,.6)'; c.shadowBlur = 10;
      for (const [x, y, r] of [[112, 56, 2], [119, 46, 3.4]]) { c.fillStyle = gr(x, y - r, x, y + r, '#fff', '#cfe0f7'); c.beginPath(); c.arc(x, y, r, 0, 7); c.fill(); }
      c.fillStyle = gr(0, 14, 0, 48, '#fff', '#c9dcf5'); c.beginPath();
      for (const [x, y, r] of [[124, 32, 8], [133, 24, 10], [146, 24, 9], [154, 32, 8], [146, 40, 8], [133, 40, 9]]) { c.moveTo(x + r, y); c.arc(x, y, r, 0, 7); }
      c.fill(); c.shadowBlur = 0; c.fillStyle = 'rgba(255,255,255,.9)'; c.beginPath(); c.ellipse(131, 18, 5, 2, -0.4, 0, 7); c.fill();
      for (let i = 0; i < 3; i++) { c.globalAlpha = 0.3 + 0.7 * Math.max(0, Math.sin(t * 4 - i * 0.9)); c.fillStyle = '#3B8EF0'; c.beginPath(); c.arc(135 + i * 8, 32, 2.5, 0, 7); c.fill(); }
      c.restore();
    },
    talk() { c.lineCap = 'round'; c.lineWidth = 3.4; c.strokeStyle = bl; c.shadowColor = bl; c.shadowBlur = 6; for (const i of [-1, 0, 1]) { const l = 6 + 7 * Math.abs(Math.sin(t * 9 + i * 1.3)); c.globalAlpha = 0.5 + 0.5 * Math.abs(Math.sin(t * 9 + i)); c.beginPath(); c.moveTo(131, 62 + i * 11); c.lineTo(131 + l, 62 + i * 11 + i * 4); c.stroke(); } },
    search() {
      const x = 118 + Math.sin(t * 2.2) * 9, y = 50 + Math.cos(t * 2.2) * 4;
      c.save(); c.translate(x, y); c.rotate(-0.2); c.lineCap = 'round';
      c.strokeStyle = '#1d2f4d'; c.lineWidth = 8; c.beginPath(); L(8, 9, 22, 23); c.stroke();
      c.strokeStyle = gr(0, 8, 0, 24, '#5b7fb5', '#2a4472'); c.lineWidth = 6; c.beginPath(); L(8, 9, 22, 23); c.stroke();
      c.strokeStyle = 'rgba(255,255,255,.35)'; c.lineWidth = 1.4; c.beginPath(); L(10, 8, 23, 21); c.stroke();
      c.strokeStyle = gr(0, 0, 12, 12, '#f4f8ff', '#8aa3c7'); c.lineWidth = 7.5; c.beginPath(); L(6.5, 7.5, 10, 11); c.stroke();
      c.shadowColor = bl; c.shadowBlur = 10; c.fillStyle = gr(-10, -10, 10, 10, 'rgba(230,246,255,.55)', 'rgba(110,185,255,.2)');
      c.beginPath(); c.arc(0, 0, 11, 0, 7); c.fill(); c.shadowBlur = 0; c.lineWidth = 3.6; c.strokeStyle = gr(-11, -11, 11, 11, '#f4f8ff', '#6f9fe0'); c.stroke();
      c.lineWidth = 1; c.strokeStyle = 'rgba(30,70,140,.5)'; c.beginPath(); c.arc(0, 0, 12.9, 0, 7); c.stroke();
      c.strokeStyle = 'rgba(255,255,255,.9)'; c.lineWidth = 2; c.beginPath(); c.arc(0, 0, 7.2, Math.PI * 1.1, Math.PI * 1.45); c.stroke();
      c.fillStyle = '#fff'; c.beginPath(); c.arc(-3.2, -4.5, 1.3, 0, 7); c.fill(); spark(4, 4, 2.6 * (0.7 + 0.3 * Math.sin(t * 6))); c.restore();
    },
    work() { gear(127, 33, 10, 8, t * 1.2); gear(111, 50, 6, 6, -t * 2 + 0.3); c.fillStyle = gr(40, 22, 40, 42, '#cfe6ff', '#5aaeff'); c.shadowColor = bl; c.shadowBlur = 6; c.globalAlpha = 0.5 + 0.5 * Math.sin(t * 4); spark(44, 32, 5.5); spark(36, 50, 3.4); },
    happy() { c.lineCap = 'round'; c.strokeStyle = bl; c.lineWidth = 3; c.shadowColor = bl; c.shadowBlur = 6; c.globalAlpha = 0.6 + 0.4 * Math.sin(t * 6); c.beginPath(); L(32, 44, 26, 40); L(40, 32, 36, 25); L(128, 44, 134, 40); L(120, 32, 124, 25); c.stroke(); c.fillStyle = '#cfe6ff'; spark(138, 26, 3.4 * (1 + 0.2 * Math.sin(t * 5))); spark(22, 28, 3); },
    proud() { const k = 1 + 0.18 * Math.sin(t * 4); c.shadowColor = bl; c.shadowBlur = 8; c.fillStyle = gr(0, 22, 0, 46, '#d8edff', '#3B9EFF'); spark(128, 34, 10 * k); spark(36, 58, 6.5 / k); spark(136, 58, 4.2 * k); },
    love() { for (let i = 0; i < 3; i++) { const p = (t * 0.5 + i / 3) % 1; c.globalAlpha = Math.min(1, (1 - p) * 1.6); heart2(112 + i * 11 + Math.sin(t * 3 + i) * 4, 72 - p * 46, 5.2 + i, Math.sin(t * 2 + i) * 0.25); } },
    surp() { c.lineCap = 'round'; c.strokeStyle = bl; c.lineWidth = 3.4; c.shadowColor = bl; c.shadowBlur = 6; c.globalAlpha = 0.65 + 0.35 * Math.sin(t * 10); c.beginPath(); L(80, 24, 80, 11); L(63, 28, 56, 18); L(97, 28, 104, 18); c.stroke(); },
    q() {
      const y = Math.sin(t * 2.6) * 2.5;
      c.save(); c.translate(124, 36 + y); c.rotate(Math.sin(t * 2) * 0.07);
      const bp = () => { c.beginPath(); c.roundRect(-15, -13, 30, 25, 10); c.moveTo(-6, 11); c.quadraticCurveTo(-8, 16, -14, 18); c.quadraticCurveTo(-3, 18, 3, 11); c.closePath(); };
      c.shadowColor = 'rgba(59,158,255,.75)'; c.shadowBlur = 12; c.fillStyle = gr(0, -13, 0, 13, '#a9dcff', '#2f86f0'); bp(); c.fill();
      c.shadowBlur = 0; c.strokeStyle = 'rgba(255,255,255,.55)'; c.lineWidth = 1.2; bp(); c.stroke();
      c.fillStyle = 'rgba(255,255,255,.28)'; c.beginPath(); c.roundRect(-12, -11, 24, 8, 5); c.fill();
      c.strokeStyle = '#fff'; c.lineWidth = 3.2; c.lineCap = 'round'; c.beginPath(); c.arc(0, -3.6, 4.6, Math.PI * 1.1, Math.PI * 2.5); c.lineTo(0, 2.4); c.stroke();
      c.fillStyle = '#fff'; c.beginPath(); c.arc(0, 7, 1.8, 0, 7); c.fill(); c.restore();
    },
    ann() {
      const k = 1 + 0.14 * Math.sin(t * 9);
      c.save(); c.translate(124, 32); c.rotate(Math.sin(t * 5) * 0.08); c.scale(k, k); c.strokeStyle = '#ff4d42'; c.shadowColor = '#ff3b30'; c.shadowBlur = 10; c.lineWidth = 3.6; c.lineCap = 'round';
      for (const [sx, sy] of [[1, 1], [-1, 1], [1, -1], [-1, -1]]) { c.beginPath(); c.moveTo(sx * 3, sy * 10); c.quadraticCurveTo(sx * 3, sy * 3, sx * 10, sy * 3); c.stroke(); }
      c.restore(); c.fillStyle = 'rgba(255,255,255,.6)';
      for (let i = 0; i < 3; i++) { const p = (t * 0.8 + i / 3) % 1; for (const d of [-1, 1]) { c.globalAlpha = (1 - p) * 0.5; c.beginPath(); c.arc(80 + d * (47 + i * 2), 54 - p * 38, 2.5 + p * 4.5, 0, 7); c.fill(); } }
    },
    zzz() {
      c.lineCap = c.lineJoin = 'round'; c.shadowColor = bl; c.shadowBlur = 7;
      for (let i = 0; i < 3; i++) {
        const p = (t * 0.35 + i / 3) % 1, z = 3.2 + i * 1.6 + p * 1.4, x = 112 + p * 18 + i * 4, y = 56 - p * 30 - i * 3;
        c.globalAlpha = Math.sin(p * Math.PI) * 0.9; c.strokeStyle = gr(x, y - z, x, y + z, '#dcefff', '#5aaeff'); c.lineWidth = 2.4 + z * 0.12;
        c.beginPath(); c.moveTo(x - z, y - z); c.lineTo(x + z, y - z); c.lineTo(x - z, y + z); c.lineTo(x + z, y + z); c.stroke();
      }
    },
    err() {
      const k = 1 + 0.06 * Math.sin(t * 10);
      c.save(); c.translate(124, 32); c.scale(k, k); c.shadowColor = 'rgba(255,90,90,.7)'; c.shadowBlur = 10; c.fillStyle = gr(0, -12, 0, 12, '#ff9d96', '#ee4d4d');
      c.beginPath(); c.arc(0, 0, 12, 0, 7); c.fill(); c.shadowBlur = 0; c.fillStyle = 'rgba(255,255,255,.3)'; c.beginPath(); c.ellipse(-3, -6.5, 6, 3, -0.5, 0, 7); c.fill();
      c.strokeStyle = '#fff'; c.lineWidth = 3.4; c.lineCap = 'round'; c.beginPath(); L(-4.5, -4.5, 4.5, 4.5); L(4.5, -4.5, -4.5, 4.5); c.stroke(); c.restore();
    },
    diz() {
      for (let i = 0; i < 3; i++) { const a = t * 4 + i * 2.094, x = 80 + Math.cos(a) * 52, y = 34 + Math.sin(a) * 10; c.globalAlpha = 0.65 + 0.35 * Math.sin(a); c.shadowColor = '#ffd54a'; c.shadowBlur = 8; c.fillStyle = gr(x, y - 6, x, y + 6, '#fff2a6', '#ffb62b'); star5(x, y, 5.6, 2.4, t * 2 + i); c.fill(); }
      c.shadowBlur = 0; c.strokeStyle = bl; c.globalAlpha = 0.4; c.lineWidth = 2; c.lineCap = 'round'; const w = Math.sin(t * 3) * 0.2; c.beginPath(); c.arc(80, 34, 58, Math.PI * 1.12 + w, Math.PI * 1.4 + w); c.stroke();
    },
    wave() { c.lineCap = 'round'; c.strokeStyle = bl; c.lineWidth = 3; c.shadowColor = bl; c.shadowBlur = 6; for (let i = 0; i < 2; i++) { c.globalAlpha = 0.3 + 0.7 * Math.abs(Math.sin(t * 6 - i)); c.beginPath(); c.arc(130, 56, 9 + i * 6, -1, 0.5); c.stroke(); } c.fillStyle = '#cfe6ff'; spark(142, 34, 3.4); }
  };

  function bodyPath(sx = 1, sy = 1) {
    const p = new Path2D();
    for (let i = 0; i <= 84; i++) {
      const a = (i / 84) * 6.2832, ca = Math.cos(a), sa = Math.sin(a), top = sa < 0, e = top ? 1.18 : 0.6, ry = top ? 47 : 40;
      const px = 49 * sx * Math.sign(ca) * Math.abs(ca) ** e;
      const py = -40 + ry * sy * Math.sign(sa) * Math.abs(sa) ** e;
      i ? p.lineTo(px, py) : p.moveTo(px, py);
    }
    p.closePath();
    return p;
  }

  function tails(dt, s) {
    const tc = Array.isArray(cfg.twintailsColors) && cfg.twintailsColors.length === 2 ? cfg.twintailsColors : ['#f4f6fb', '#cbd5e1'];
    TL.forEach((T, i) => {
      const d = i ? 1 : -1;
      const tg = 0.12 + Math.sin(t * 1.6 + i * 1.1) * 0.06 + P.tilt * d * 1.1 + Math.min(0.45, Math.abs(P.jv) * 0.0035) + ((s.bounce || s.hop) ? Math.sin(t * (s.bounce ? 6 : 4.2) + i) * 0.14 : 0);
      T.v += (-70 * (T.a - tg) - 7.5 * T.v) * dt;
      T.a += T.v * dt;
      c.save(); c.translate(d * 41, -62); c.scale(d, 1); c.rotate(-T.a);
      const B = [[0, 0], [20, -8], [36, 30], [26, 74]];
      const bz = (u, k) => { const m = 1 - u; return m * m * m * B[0][k] + 3 * m * m * u * B[1][k] + 3 * m * u * u * B[2][k] + u * u * u * B[3][k]; };
      const dz = (u, k) => { const m = 1 - u; return 3 * m * m * (B[1][k] - B[0][k]) + 6 * m * u * (B[2][k] - B[1][k]) + 3 * u * u * (B[3][k] - B[2][k]); };
      const N = 20, pt = [];
      for (let j = 0; j <= N; j++) {
        const u = j / N, dx = dz(u, 0), dy = dz(u, 1), l = Math.hypot(dx, dy) || 1;
        pt.push({ x: bz(u, 0), y: bz(u, 1), nx: -dy / l, ny: dx / l, w: (1 - Math.pow(u, 1.5)) * (0.62 + 0.38 * Math.sin(Math.min(1, u * 2.4) * 1.5708)) * 9.5 });
      }
      c.beginPath(); pt.forEach((q, j) => c[j ? 'lineTo' : 'moveTo'](q.x + q.nx * q.w, q.y + q.ny * q.w));
      for (let j = N; j >= 0; j--) { const q = pt[j]; c.lineTo(q.x - q.nx * q.w, q.y - q.ny * q.w); }
      c.closePath(); c.fillStyle = gr(0, -6, 0, 74, tc[0], tc[1]); c.fill();
      c.lineCap = 'round';
      for (const [o, col, lw, a0, a1] of [[-0.5, 'rgba(51,65,85,.22)', 1.3, 0.05, 0.95], [0.1, 'rgba(51,65,85,.18)', 1.2, 0.05, 0.85], [0.55, 'rgba(255,255,255,.55)', 1.7, 0.08, 0.6]]) {
        c.strokeStyle = col; c.lineWidth = lw; c.beginPath();
        for (let j = Math.round(a0 * N); j <= Math.round(a1 * N); j++) { const q = pt[j]; c[j === Math.round(a0 * N) ? 'moveTo' : 'lineTo'](q.x + q.nx * q.w * o, q.y + q.ny * q.w * o); }
        c.stroke();
      }
      const q = pt[1]; c.save(); c.translate(q.x, q.y); c.rotate(Math.atan2(dz(0.07, 1), dz(0.07, 0)));
      c.fillStyle = '#24344f'; c.beginPath(); c.roundRect(-3.2, -(q.w + 1.4), 6.4, 2 * q.w + 2.8, 3); c.fill();
      c.fillStyle = cfg.blushColor || '#ff8fb1'; c.beginPath(); c.arc(0, -(q.w + 1.4) + 1.6, 1.7, 0, 7); c.fill();
      c.restore(); c.restore();
    });
  }

  function eye(side, x, y, type, op, sc) {
    c.fillStyle = c.strokeStyle = dark; c.lineWidth = 3.2; c.lineCap = 'round'; c.lineJoin = 'round';
    if (type === 'wink' && side > 0) type = '<';
    if (type === 'ang') {
      c.save();
      const ix = x - side * 9, ox = x + side * 9, ln = u => y - 2 - 6.5 * ((u - ix) / (ox - ix)), xa = x - side * 13.5, xb = x + side * 13.5;
      c.beginPath(); c.moveTo(xa, ln(xa)); c.lineTo(xb, ln(xb)); c.lineTo(xb, y + 12); c.lineTo(xa, y + 12); c.closePath(); c.clip();
      c.beginPath(); c.ellipse(x, y, 5.4 * sc, 7.8 * sc * Math.max(0.08, op), 0, 0, 7); c.fill(); c.restore();
      return;
    }
    if (type === 'o' || type === 'wink') { c.beginPath(); c.ellipse(x, y, 5.2 * sc, 7.8 * sc * Math.max(0.08, op), 0, 0, 7); c.fill(); }
    else if (type === 'arc') { c.beginPath(); c.arc(x, y + 4, 7, Math.PI * 1.1, Math.PI * 1.9); c.stroke(); }
    else if (type === 'line') { c.beginPath(); c.arc(x, y - 3, 7, 0.15 * Math.PI, 0.85 * Math.PI); c.stroke(); }
    else if (type === 'x') { c.beginPath(); L(x - 6, y - 6, x + 6, y + 6); L(x + 6, y - 6, x - 6, y + 6); c.stroke(); }
    else if (type === '<') { c.beginPath(); L(x + 5, y - 5, x - 5, y); L(x - 5, y, x + 5, y + 5); c.stroke(); }
    else if (type === 'spiral') { c.beginPath(); for (let a = 0; a < 14; a += 0.4) { const r = a * 0.55, px = x + Math.cos(a + t * 8) * r, py = y + Math.sin(a + t * 8) * r; a ? c.lineTo(px, py) : c.moveTo(px, py); } c.stroke(); }
  }

  function mouth(m) {
    c.fillStyle = c.strokeStyle = dark; c.lineWidth = 2.6; c.lineCap = 'round'; c.beginPath();
    if (m === 'dot') { L(-3, 0, 3, 0); c.stroke(); }
    else if (m === 'O') { c.ellipse(0, 1, 3.2, 4.2, 0, 0, 7); c.fill(); }
    else if (m === 'smile') { c.arc(0, -3, 5.5, 0.15 * Math.PI, 0.85 * Math.PI); c.stroke(); }
    else if (m === 'frown') { c.arc(0, 6, 5, 1.15 * Math.PI, 1.85 * Math.PI); c.stroke(); }
    else if (m === 'talk') { const o = 0.8 + Math.abs(Math.sin(t * 14)) * 4; c.ellipse(0, 1, 4.5, o, 0, 0, 7); c.fill(); c.fillStyle = '#FF9DB8'; c.beginPath(); c.ellipse(0, o * 0.55 + 1, 2.8, o * 0.4, 0, 0, 7); c.fill(); }
    else if (m === 'grr') { c.lineJoin = 'round'; for (let i = 0; i <= 5; i++) { const px = -9 + i * 3.6, py = (i % 2 ? -2.4 : 1.8) + Math.sin(t * 34 + i * 1.7) * 0.5; i ? c.lineTo(px, py) : c.moveTo(px, py); } c.stroke(); }
    else if (m === 'slot') { c.roundRect(-17, -3, 34, 9, 4.5); c.fill(); }
    else if (m === 'eat') {
      const o = Math.max(0, Math.min(1.1, P.mo)), w = 3.4 + o * 6.4, h = 0.9 + o * 10.8, cy = 1 + h * 0.4;
      c.ellipse(0, cy, w, h, 0, 0, 7); c.fill();
      if (o > 0.22) { c.save(); c.beginPath(); c.ellipse(0, cy, w, h, 0, 0, 7); c.clip(); c.fillStyle = '#FF9DB8'; c.beginPath(); c.ellipse(0, cy + h * 0.58, w * 0.8, h * 0.5, 0, 0, 7); c.fill(); c.restore(); }
    }
  }

  function hand(d, rot, oy, g) { c.save(); c.translate(d * 50, -28 + oy); c.rotate(rot * d); c.beginPath(); c.ellipse(0, 0, 6.5, 12, 0, 0, 7); c.fillStyle = g; c.fill(); c.restore(); }

  // Proyección 3D esferoide para sombreros y accesorios
  const ringR = y => Math.pow(1 - Math.pow(Math.min(1, Math.abs(y)), 2.7), 1 / 2.7);
  const surf = (y, lon, sc = 1) => { const r = ringR(y) * sc; return [r * Math.sin(lon), y, r * Math.cos(lon)]; };
  function rot3(p) {
    const yw = P.lx * 0.5, pt = -0.30 + (-P.ly * 0.4) * 0.4, cy = Math.cos(yw), sy = Math.sin(yw), cp = Math.cos(pt), sp = Math.sin(pt);
    const x1 = p[0] * cy + p[2] * sy, z1 = -p[0] * sy + p[2] * cy;
    return [x1, p[1] * cp + z1 * sp, -p[1] * sp + z1 * cp];
  }
  const pj = p => { const r = rot3(p); return { x: r[0] * 49, y: -40 - r[1] * (r[1] > 0 ? 47 : 40), z: r[2] }; };
  const frontArc = (y, sc) => { const a = []; for (let i = 0; i <= 100; i++) { const q = pj(surf(y, -Math.PI + (i / 100) * 6.2832, sc)); if (q.z >= -0.02) a.push(q); } return a.sort((p, q) => p.x - q.x); };
  const capClip = (y, sc) => { const a = frontArc(y, sc), p = new Path2D(), L0 = a[0], R0 = a[a.length - 1]; p.moveTo(L0.x - 49, L0.y); a.forEach(q => p.lineTo(q.x, q.y)); p.lineTo(R0.x + 49, R0.y); p.lineTo(110, -180); p.lineTo(-110, -180); p.closePath(); return p; };
  const poly = (f, a) => { c.fillStyle = f; c.beginPath(); a.forEach((q, i) => c[i ? 'lineTo' : 'moveTo'](q.x, q.y)); c.closePath(); c.fill(); };
  const ci = (x, y, r, f) => { c.fillStyle = f; c.beginPath(); c.arc(x, y, r, 0, 7); c.fill(); };
  const pom = (x, y, r) => { ci(x, y, r, gr(x, y - r, x, y + r, '#fff', '#d3dbea')); ci(x - r * 0.3, y - r * 0.35, r * 0.28, 'rgba(255,255,255,.95)'); };
  const band = (y0, y1, sc, f) => poly(f, [...frontArc(y0, sc), ...frontArc(y1, sc).reverse()]);
  const hull = a => {
    const p = a.map(q => ({ x: q.x, y: q.y })).sort((u, v) => u.x - v.x || u.y - v.y), cr = (o, a2, b2) => (a2.x - o.x) * (b2.y - o.y) - (a2.y - o.y) * (b2.x - o.x), lo = [], up = [];
    for (const q of p) { while (lo.length > 1 && cr(lo[lo.length - 2], lo[lo.length - 1], q) <= 0) lo.pop(); lo.push(q); }
    for (let i = p.length - 1; i >= 0; i--) { const q = p[i]; while (up.length > 1 && cr(up[up.length - 2], up[up.length - 1], q) <= 0) up.pop(); up.push(q); }
    lo.pop(); up.pop(); return lo.concat(up);
  };
  const ringP = (y, R, ox = 0, oz = 0, n = 40) => { const a = []; for (let i = 0; i < n; i++) { const k = (i / n) * 6.2832; a.push(pj([ox + R * Math.sin(k), y, oz + R * Math.cos(k)])); } return a; };
  const frontP = (y, R, ox = 0, oz = 0) => { const a = []; for (let i = 0; i <= 72; i++) { const k = (i / 72) * 6.2832; if (rot3([Math.sin(k), 0, Math.cos(k)])[2] >= 0) a.push(pj([ox + R * Math.sin(k), y, oz + R * Math.cos(k)])); } return a.sort((p, q) => p.x - q.x); };
  const strokeP = a => { c.beginPath(); a.forEach((q, i) => c[i ? 'lineTo' : 'moveTo'](q.x, q.y)); c.stroke(); };
  const phU = (d, y) => d * Math.pow(Math.cos(Math.asin(Math.min(1, Math.pow(y, 1 / 1.18)))), 1.18);
  const phBand = () => { const a = [], f0 = 0.2118, n = 28; for (let i = 0; i <= n; i++) { const ph = f0 + ((Math.PI - 2 * f0) * i) / n, ca = Math.cos(ph), sa = Math.sin(ph); a.push(pj([Math.sign(ca) * Math.pow(Math.abs(ca), 1.18) * 1.045, Math.pow(sa, 1.18) * 1.045, 0])); } return a; };
  function phCup(d) {
    const q = pj([phU(d, 0.16), 0.16, 0]), w = 3.6 + 3.4 * Math.max(0, Math.min(1, 0.4 + q.z));
    c.save(); c.translate(q.x + d * 2.2, q.y);
    c.fillStyle = gr(-w, -12, w, 12, '#3a4152', '#171a23'); c.beginPath(); c.ellipse(0, 0, w, 11.5, 0, 0, 7); c.fill();
    c.globalAlpha = 0.55 + 0.45 * Math.sin(t * 3); c.strokeStyle = BODY; c.lineWidth = 1.4; c.beginPath(); c.ellipse(0, 0, w * 0.68, 9, 0, 0, 7); c.stroke(); c.globalAlpha = 1;
    c.fillStyle = BODY; for (let i = 0; i < 3; i++) { const h = 3 + 6 * Math.abs(Math.sin(t * 6 + i * 1.4 + d)); c.fillRect(-w * 0.42 + i * w * 0.42, -h / 2, Math.max(1, w * 0.28), h); }
    c.fillStyle = 'rgba(255,255,255,.16)'; c.beginPath(); c.ellipse(-d * w * 0.3, -7, w * 0.25, 3.6, 0, 0, 7); c.fill(); c.restore();
  }
  const fluff = (x, y, r, n = 7, sp = 0.6) => { for (let i = 0; i < n; i++) { const an = (i / n) * 6.2832 + t * sp; ci(x + Math.cos(an) * r * 0.66, y + Math.sin(an) * r * 0.62, r * 0.58, '#eef2fa'); } pom(x, y, r * 0.94); };
  const flake = (x, y, r, rot) => { c.save(); c.translate(x, y); c.rotate(rot); c.beginPath(); for (let i = 0; i < 3; i++) { c.moveTo(0, -r); c.lineTo(0, r); c.rotate(1.0472); } c.stroke(); c.restore(); };

  function crownPart(side) {
    const N = 96, sg = [];
    for (let i = 0; i <= N; i++) {
      const lon = -Math.PI + (i / N) * 6.2832, ph = ((lon + Math.PI) / 6.2832) * 8, f = ph - Math.floor(ph), sp = Math.pow(Math.max(0, 1 - Math.abs(f - 0.5) * 2), 1.6);
      const b = pj(surf(0.46, lon, 1.06)), tp = surf(0.66, lon, 1.06); tp[1] = 0.66 + 0.4 * sp; tp[0] *= 1 - 0.08 * sp; tp[2] *= 1 - 0.08 * sp; sg.push({ b, t: pj(tp) });
    }
    const k = sg.filter(q => side > 0 ? q.b.z >= 0 : q.b.z < 0.02).sort((a, b) => a.b.x - b.b.x);
    if (k.length < 2) return;
    poly(side > 0 ? gr(0, -110, 0, -58, '#ffe58a', '#dd9a0c') : '#a9730d', [...k.map(q => q.t), ...k.map(q => q.b).reverse()]);
    if (side > 0) for (let m = 0; m < 8; m++) { const g = pj(surf(0.56, -Math.PI + ((m + 0.5) / 8) * 6.2832, 1.07)); if (g.z > 0.18) ci(g.x, g.y, 2.4, ['#ef4444', '#3b82f6', '#22c55e', '#a855f7'][m % 4]); }
  }
  function ears() {
    for (const d of [-1, 1]) {
      const b = pj([d * 0.38, 0.93, 0]);
      c.save(); c.translate(b.x, b.y + 4); c.rotate(d * 0.16);
      c.fillStyle = BODY; c.beginPath(); c.ellipse(0, -15, 7.4, 16.5, 0, 0, 7); c.fill();
      c.strokeStyle = 'rgba(0,0,0,.14)'; c.lineWidth = 1; c.stroke();
      c.fillStyle = '#ffb3c8'; c.beginPath(); c.ellipse(0, -14, 3.4, 11.5, 0, 0, 7); c.fill(); c.restore();
    }
  }
  function outBack() {
    if (OUT === 'bunny') ears();
    else if (OUT === 'crown') crownPart(-1);
    else if (OUT === 'phones') for (const d of [-1, 1]) { if (pj([phU(d, 0.16), 0.16, 0]).z < -0.02) phCup(d); }
  }

  const OF = {
    beanie() {
      const a = frontArc(0.5, 1.06), Lq = a[0], Rq = a[a.length - 1], cr = pj([0, 1.06, 0]), sw = Math.sin(t * 2.4), tx = cr.x + sw * 2.2, ty = cr.y - 7;
      c.beginPath(); c.moveTo(Lq.x - 1, Lq.y); c.bezierCurveTo(Lq.x - 6, Lq.y - 26, tx - 30, ty - 3, tx, ty - 3);
      c.bezierCurveTo(tx + 30, ty - 3, Rq.x + 6, Rq.y - 26, Rq.x + 1, Rq.y); a.slice().reverse().forEach(q => c.lineTo(q.x, q.y)); c.closePath();
      c.fillStyle = gr(Lq.x, -138, Rq.x, -62, '#8cc0ff', '#2152c4'); c.fill(); c.save(); c.clip();
      for (const [y, w, f] of [[0.74, 7, '#f4f7ff'], [0.9, 4, '#ffd84d']]) { c.strokeStyle = f; c.lineWidth = w; c.beginPath(); frontArc(y, 1.06).forEach((q, i) => c[i ? 'lineTo' : 'moveTo'](q.x, q.y - 2)); c.stroke(); }
      const rg = c.createRadialGradient(-18, -100, 2, -8, -84, 66); rg.addColorStop(0, 'rgba(255,255,255,.42)'); rg.addColorStop(1, 'rgba(255,255,255,0)'); c.fillStyle = rg; c.fillRect(-90, -160, 180, 130); c.restore();
      band(0.54, 0.34, 1.08, gr(0, -72, 0, -46, '#5b93f2', '#1f4fbf'));
      const sp = pj([0, 0.44, 1.08]); c.fillStyle = gr(sp.x, sp.y - 7, sp.x, sp.y + 7, '#fff3b0', '#ffc21a'); star5(sp.x, sp.y, 6.2, 2.8, Math.sin(t * 1.6) * 0.25); c.fill();
      const px = tx, py = ty - 5 - Math.abs(sw) * 1.6; ci(px, py, 6.6, gr(px, py - 6.6, px, py + 6.6, '#fff', '#c9d6f0')); ci(px - 2.2, py - 2.4, 2.1, 'rgba(255,255,255,.95)');
    },
    party() {
      const A = 0.9, R = 0.44, ax = -0.1, ay = 1.44, ap = pj([ax, ay, 0]); ap.x += Math.sin(t * 3) * 0.8;
      const H = hull([...ringP(A, R), ap]);
      c.beginPath(); H.forEach((q, i) => c[i ? 'lineTo' : 'moveTo'](q.x, q.y)); c.closePath(); c.fillStyle = gr(0, ap.y, 0, ap.y + 34, '#ff9bd0', '#c8228a'); c.fill();
      c.save(); c.clip();
      for (const [u, col] of [[0.26, '#ffe066'], [0.52, '#5ce1e6'], [0.78, '#ffe066']]) { const fr = frontP(A + (ay - A) * u, R * (1 - u), ax * u); if (fr.length > 1) { c.strokeStyle = col; c.lineWidth = 3; strokeP(fr); } }
      c.restore();
      const fr = frontP(A, R); c.lineCap = 'round'; c.strokeStyle = '#ffd84d'; c.lineWidth = 3; strokeP(fr);
      c.save(); c.shadowColor = '#ffd84d'; c.shadowBlur = 7; c.fillStyle = gr(ap.x, ap.y - 5, ap.x, ap.y + 4, '#fff6c0', '#ffc21a'); star5(ap.x, ap.y - 1, 5.2, 2.3, t * 1.5); c.fill(); c.restore();
    },
    witch() {
      const A = 0.84, br = hull(ringP(A, 0.96)), B1 = 1.22, ap = pj([0.2, 1.56, 0]); ap.x += Math.sin(t * 1.7) * 1.2;
      poly(gr(0, -100, 0, -62, '#6d28d9', '#2e1065'), br);
      c.strokeStyle = 'rgba(200,170,255,.55)'; c.lineWidth = 1.3; c.beginPath(); br.forEach((q, i) => c[i ? 'lineTo' : 'moveTo'](q.x, q.y)); c.closePath(); c.stroke();
      for (const sg of [hull([...ringP(A + 0.02, 0.5), ...ringP(B1, 0.2, 0.04)]), hull([...ringP(B1, 0.2, 0.04), ap])]) poly(gr(-24, 0, 24, 0, '#9a6cfb', '#2a0f5e'), sg);
      const fr = frontP(0.96, 0.43); c.lineCap = 'round'; c.strokeStyle = '#ffa24a'; c.lineWidth = 4.6; strokeP(fr);
      const bk = pj([0, 0.96, 0.43]); if (bk.z > 0) { c.fillStyle = gr(bk.x, bk.y - 6, bk.x, bk.y + 6, '#ffe58a', '#d99a0b'); c.beginPath(); c.roundRect(bk.x - 4.6, bk.y - 4, 9.2, 8, 2); c.fill(); }
    },
    santa() {
      const a = frontArc(0.52, 1.06), Lq = a[0], Rq = a[a.length - 1], cr = pj([0, 1.05, 0]), sw = Math.sin(t * 2.2), tip = { x: cr.x + 34 + sw * 3, y: cr.y + 14 + Math.abs(sw) * 2 }, pk = { x: cr.x + 6, y: cr.y - 17 };
      c.beginPath(); c.moveTo(Lq.x, Lq.y); c.bezierCurveTo(Lq.x - 3, Lq.y - 26, pk.x - 22, pk.y, pk.x, pk.y);
      c.quadraticCurveTo(tip.x - 4, pk.y - 3, tip.x, tip.y); c.quadraticCurveTo(tip.x - 8, tip.y - 14, pk.x + 10, pk.y + 16);
      c.bezierCurveTo(Rq.x + 3, pk.y + 26, Rq.x + 4, Rq.y - 18, Rq.x, Rq.y); a.slice().reverse().forEach(q => c.lineTo(q.x, q.y)); c.closePath();
      c.fillStyle = gr(Lq.x, -130, Rq.x + 30, -60, '#ff5a5a', '#a31515'); c.fill();
      const fur = frontArc(0.52, 1.09); c.strokeStyle = '#e6ecf6'; c.lineWidth = 9; c.beginPath(); fur.forEach((q, i) => c[i ? 'lineTo' : 'moveTo'](q.x, q.y - 1)); c.stroke();
      for (let i = 0; i < fur.length; i += 3) { const q = fur[i], y = q.y - 1.5 + (i % 2 ? 1.6 : -1); ci(q.x, y, 5.2, gr(q.x, y - 6, q.x, y + 5, '#fff', '#dbe3f0')); }
      fluff(tip.x, tip.y + 3, 5.8, 7, 0.9);
      c.strokeStyle = '#fff'; c.lineWidth = 1.2; for (let k = 0; k < 3; k++) { const p = (t * 0.22 + k / 3) % 1; c.globalAlpha = Math.sin(p * Math.PI) * 0.85; flake(cr.x - 20 + k * 18, -124 + p * 50, 2.5, t * 1.5 + k); } c.globalAlpha = 1;
    },
    pumpkin() {
      const a = EP[0], b = EP[1], fx = Math.max(0.3, (a[2] + b[2]) / 2), fy = Math.max(0.4, (a[3] + b[3]) / 2), mx = (a[0] + b[0]) / 2, my = (a[1] + b[1]) / 2 - 33, fl = 0.6 + 0.4 * Math.sin(t * 9), op = Math.max(0.15, OPV), mo = Math.min(1, Math.max(0, P.mo));
      c.save(); c.translate(mx, my); c.rotate(-0.05); c.scale(fx, fy);
      for (const [dx, rx] of [[-16, 10], [16, 10], [-8, 12.5], [8, 12.5], [0, 14]]) { c.fillStyle = gr(0, -18, 0, 18, '#ffa63a', '#e0540a'); c.beginPath(); c.ellipse(dx, 0, rx, 18, 0, 0, 7); c.fill(); }
      c.fillStyle = `rgba(255,${200 + 30 * fl | 0},70,${0.8 + 0.2 * fl})`;
      for (const d of [-1, 1]) { c.save(); c.translate(d * 10.5, -3); c.scale(1, op); c.beginPath(); c.moveTo(-4.6, 3.6); c.lineTo(0, -5); c.lineTo(4.6, 3.6); c.closePath(); c.fill(); c.restore(); }
      c.beginPath(); c.moveTo(-10, 4); c.lineTo(-6, 2.6); c.lineTo(-3.6, 5.4); c.lineTo(0, 2.6); c.lineTo(3.6, 5.4); c.lineTo(6, 2.6); c.lineTo(10, 4); c.quadraticCurveTo(0, 10 + mo * 9, -10, 4); c.closePath(); c.fill();
      c.lineCap = 'round'; c.strokeStyle = '#3f7d20'; c.lineWidth = 4; c.beginPath(); c.moveTo(0, -16); c.quadraticCurveTo(1, -20, 4, -23); c.stroke();
      c.restore();
    },
    halo() {
      const q = pj([0, 1.2, 0]), cx = q.x, cy = q.y - 7 + Math.sin(t * 2) * 1.8;
      c.save(); c.shadowColor = '#ffd84d'; c.shadowBlur = 11 + 3 * Math.sin(t * 3); c.lineWidth = 4.4; c.strokeStyle = gr(cx - 27, cy, cx + 27, cy, '#ffe58a', '#f5a40c'); c.beginPath(); c.ellipse(cx, cy, 26, 7.6, 0, 0, 7); c.stroke(); c.restore();
      c.fillStyle = '#fff6c4'; for (let i = 0; i < 3; i++) { const an = t * 1.8 + i * 2.094; spark(cx + Math.cos(an) * 26, cy + Math.sin(an) * 7.6, 2.3 + Math.sin(an * 2) * 0.8); }
    },
    propeller() {
      c.save(); c.clip(capClip(0.62, 1.05)); c.clip(bodyPath(1.06, 1.06));
      c.fillStyle = gr(-40, -120, 40, -52, '#59abff', '#1a49ad'); c.fill(bodyPath(1.06, 1.06));
      const cols = ['#ffd84d', '#ff5c66'];
      for (let k = 0; k < 6; k++) { const lon = -Math.PI + (k * Math.PI) / 3 + 0.5; c.strokeStyle = cols[k % 2]; c.lineWidth = 4; c.beginPath(); let st = 0; for (let s = 0; s <= 10; s++) { const q = pj(surf(0.6 + (0.46 * s) / 10, lon, 1.06)); if (q.z > 0) { c[st ? 'lineTo' : 'moveTo'](q.x, q.y); st = 1; } else st = 0; } c.stroke(); }
      c.restore();
      band(0.62, 0.5, 1.07, gr(0, -72, 0, -60, '#2c64d8', '#12378a'));
      const t0 = pj([0, 1.07, 0]), hx = t0.x, hy = t0.y - 11, w = Math.cos(t * 14);
      c.fillStyle = '#d9dde6'; c.beginPath(); c.roundRect(hx - 1.6, hy, 3.2, 10, 1); c.fill();
      for (const d of [-1, 1]) { const bw = Math.max(2, Math.abs(w) * 16); c.fillStyle = d > 0 ? '#ff5c66' : '#ffd84d'; c.beginPath(); c.ellipse(hx + d * bw * 0.5, hy + Math.sin(t * 14) * 0.8, bw * 0.5, 3, 0, 0, 7); c.fill(); }
      ci(hx, hy, 3, '#ffd84d');
    },
    phones() {
      const pts = phBand(); c.lineJoin = c.lineCap = 'round'; c.strokeStyle = '#262b38'; c.lineWidth = 4.6; strokeP(pts);
      c.strokeStyle = 'rgba(255,255,255,.22)'; c.lineWidth = 1.3; strokeP(pts.map(q => ({ x: q.x, y: q.y - 1.2 })));
      for (const d of [-1, 1]) { if (pj([phU(d, 0.16), 0.16, 0]).z >= -0.02) phCup(d); }
    }
  };

  function outfit() {
    if (OUT === 'none') return;
    c.save(); c.lineJoin = c.lineCap = 'round';
    if (OF[OUT]) OF[OUT]();
    else if (OUT === 'crown') crownPart(1);
    else if (OUT === 'shades' || OUT === 'round') {
      const sh = OUT === 'shades'; c.strokeStyle = sh ? '#0f1116' : '#d9a25f'; c.lineWidth = sh ? 3 : 2.4;
      c.beginPath(); L(EP[0][0] + 11 * EP[0][2], EP[0][1] - 3, EP[1][0] - 11 * EP[1][2], EP[1][1] - 3); c.stroke();
      for (const q of EP) {
        c.save(); c.translate(q[0], q[1]); c.scale(q[2], q[3]); c.beginPath();
        if (sh) { c.roundRect(-13, -9, 26, 18, 7); c.fillStyle = 'rgba(15,17,22,.94)'; c.fill(); c.fillStyle = 'rgba(255,255,255,.3)'; c.beginPath(); c.moveTo(-8, -3); c.lineTo(-3, -7); c.lineTo(0, -7); c.lineTo(-6, -1); c.fill(); }
        else { c.arc(0, 0, 12, 0, 7); c.fillStyle = 'rgba(190,225,255,.12)'; c.fill(); c.stroke(); c.strokeStyle = 'rgba(255,255,255,.6)'; c.lineWidth = 1.4; c.beginPath(); c.arc(0, 0, 9, 3.6, 4.5); c.stroke(); }
        c.restore();
      }
    } else if (OUT === 'bow') {
      const a = pj(surf(0.86, 0.55, 1.02)), sq = Math.max(0.45, Math.cos(0.55 + P.lx * 0.5));
      c.save(); c.translate(a.x, a.y); c.rotate(0.35 + P.lx * 0.15); c.scale(sq, 1);
      for (const d of [-1, 1]) { c.fillStyle = gr(0, -10, 0, 10, '#ff9bd0', '#e0307f'); c.beginPath(); c.moveTo(0, 0); c.bezierCurveTo(d * 6, -11, d * 17, -8, d * 15, 0); c.bezierCurveTo(d * 17, 8, d * 6, 11, 0, 0); c.fill(); }
      ci(0, 0, 3.6, gr(0, -4, 0, 4, '#ffc0de', '#c2185b')); c.restore();
    } else if (OUT === 'scarf') {
      band(-0.34, -0.66, 1.05, gr(0, -26, 0, -12, '#f87171', '#b91c1c'));
      const k = pj(surf(-0.5, -0.55, 1.06));
      if (k.z > 0) { c.fillStyle = gr(0, k.y, 0, k.y + 26, '#ef4444', '#b91c1c'); c.beginPath(); c.roundRect(k.x - 7, k.y - 2, 14, 28, 5); c.fill(); c.fillStyle = 'rgba(255,255,255,.8)'; c.fillRect(k.x - 7, k.y + 10, 14, 3); ci(k.x, k.y, 7, gr(0, k.y - 7, 0, k.y + 7, '#f87171', '#b91c1c')); }
    }
    c.restore();
  }

  function drawChar(s, k, dt, op, breath, sh, bob, allowBadges = true) {
    c.clearRect(-20, -20, 200, 200); c.globalAlpha = 1;
    c.save(); c.translate(80 + sh, 128 + bob * 0.5); c.rotate(P.tilt); c.scale(1 + P.sq, breath * P.sy * (1 - P.sq * 1.1));
    c.shadowColor = BODY !== '#fff' && BODY !== '#ffffff'
      ? BODY + 'b3'
      : `rgba(${((P.r + 510) / 3) | 0},${((P.g + 510) / 3) | 0},${((P.b + 510) / 3) | 0},${Math.min(1, 0.6 * P.gi)})`;
    c.shadowBlur = 38;

    const A = s.arms || [0.5, 0, 0.5, 0], tA = [...A];
    if (s.wave) { tA[2] = 0.9 + Math.sin(t * 9) * 0.35; tA[3] = -20; }
    if (s.flap) { tA[0] = 0.7 + Math.sin(t * 10) * 0.5; tA[2] = 0.7 + Math.sin(t * 10 + 2) * 0.5; }
    const ka = Math.min(1, k * 1.5);
    P.al += (tA[0] - P.al) * ka; P.ll += (tA[1] - P.ll) * ka; P.ar += (tA[2] - P.ar) * ka; P.lr += (tA[3] - P.lr) * ka;

    c.save(); c.shadowBlur = 0; outBack(); c.restore();
    if (TAILS) { c.save(); c.shadowBlur = 0; tails(dt, s); c.restore(); }
    hand(-1, P.al, P.ll, BODY); hand(1, P.ar, P.lr, BODY);
    const bp = bodyPath(); c.fillStyle = BODY; c.fill(bp); c.fill(bp); c.shadowBlur = 0;

    const HY = P.lx * 0.5, HP = -P.ly * 0.4;
    const fp = (yw, pt) => { const cp = Math.cos(pt + HP); return { x: Math.sin(yw + HY) * cp * 49, y: -40 - Math.sin(pt + HP) * 40, fx: Math.max(0.25, Math.cos(yw + HY)), fy: Math.max(0.3, cp), sk: -Math.sin(yw + HY) * Math.sin(pt + HP) * 0.6 }; };

    if (P.bl > 0.02) for (const d of [-1, 1]) {
      const f = fp(d * 0.8, -0.42), vis = Math.min(1, Math.max(0, (Math.cos(d * 0.8 + HY) - 0.3) * 4));
      c.globalAlpha = 0.5 * vis * P.bl; c.fillStyle = s.blc || cfg.blushColor || '#FF9DB8';
      c.beginPath(); c.ellipse(f.x, f.y, 6.5 * f.fx, 3.8 * f.fy, f.sk, 0, 7); c.fill();
    }
    c.globalAlpha = 1;

    for (const d of [-1, 1]) {
      const f = fp(d * 0.43, -0.12), x = f.x, y = f.y;
      c.save(); c.translate(x, y); c.transform(1, f.sk, 0, 1, 0, 0); c.scale(f.fx, f.fy); eye(d, 0, 0, s.e, op, P.big); c.restore();
      EP[d < 0 ? 0 : 1] = [x, y, f.fx, f.fy];
      if (P.br > 0.05) { c.strokeStyle = dark; c.lineWidth = 3.2; c.beginPath(); L(x - d * 7 * f.fx, y - (11 - P.br * 5) * f.fy, x + d * 8 * f.fx, y - (11 + P.br * 3.2) * f.fy); c.stroke(); }
    }

    { const f = fp(0, -0.34), a = 1 - Math.exp(-dt * 16); ML.x += (f.x - ML.x) * a; ML.y += (f.y - ML.y) * a; c.save(); c.translate(ML.x, ML.y); c.transform(1, f.sk, 0, 1, 0, 0); c.scale(f.fx, f.fy); mouth(s.m); c.restore(); }
    OPV = op; outfit();

    if (allowBadges && typeof hooks.getBadges === 'function') {
      const b = hooks.getBadges() || {};
      if (b.unread || b.approve) {
        c.save(); c.translate(-38, -66);
        c.shadowColor = b.approve ? 'rgba(255,179,0,0.85)' : 'rgba(147,51,234,0.85)';
        c.shadowBlur = 12; c.fillStyle = b.approve ? '#ffb300' : '#8b5cf6';
        c.beginPath(); c.arc(0, 0, 11, 0, Math.PI * 2); c.fill();
        c.shadowBlur = 0; c.strokeStyle = '#14141c'; c.lineWidth = 2.2; c.stroke();
        if (b.approve) {
          c.fillStyle = '#14141c'; c.font = 'bold 12px sans-serif'; c.textAlign = 'center'; c.textBaseline = 'middle'; c.fillText('!', 0, 0.8);
        } else {
          c.fillStyle = '#ffffff';
          for (let di = -1; di <= 1; di++) { c.beginPath(); c.arc(di * 4.2, 0.5, 1.35, 0, Math.PI * 2); c.fill(); }
        }
        c.restore();
      }
    }
    c.restore();
  }

  function frameGap(now) {
    if (typeof hooks.frameGap === 'function') return hooks.frameGap(now, { cur, base, eatOn: !!EAT.on, lastAct });
    if (EAT.on) return 0;
    if (base === 'sleeping') return 66;
    return now - lastAct > 2200 ? 33 : 0;
  }

  function startFrameLoop() {
    if (paused || hidden || !c) return;
    if (!frameRaf) frameRaf = requestAnimationFrame(loopTick);
  }
  function stopFrameLoop() {
    if (frameRaf) { cancelAnimationFrame(frameRaf); frameRaf = null; }
  }

  function wakeFromRest() {
    lastAct = performance.now();
    if (isResting) {
      isResting = false;
      clearTimeout(restBlinkTimer);
      hooks.onRestChange?.(false);
      startFrameLoop();
      avatars.start();
    }
  }

  function renderFrame(now) {
    if (!c) return;
    const dt = Math.min(0.04, Math.max(0.001, (now - last) / 1000));
    last = now; t = now / 1000;
    const s = X[cur] || X.idle, k = 1 - Math.exp(-dt * 9);
    const TK = cfg.enableEyeTracking === false ? 0 : (s.trk ?? 1);
    let [tx, ty] = cur === 'look' ? ov : [mx * TK, my * TK];
    if (s.lk) [tx, ty] = s.lk;
    if (s.scan) tx = Math.sin(t * 7);
    if (EAT.on) { tx = EAT.gx; ty = EAT.gy; }
    P.lx += (tx - P.lx) * k; P.ly += (ty - P.ly) * k;
    const g = s.g || BL, gi = s.gi || 1;
    P.r += (g[0] - P.r) * k; P.g += (g[1] - P.g) * k; P.b += (g[2] - P.b) * k; P.gi += (gi - P.gi) * k;
    P.tilt += ((s.t || 0) + (s.wob ? Math.sin(t * 3) * 0.13 : 0) + P.lx * 0.035 - P.tilt) * k;
    P.bl += ((s.blush ? 1 : 0) - P.bl) * k; P.sy += ((s.sy || 1) - P.sy) * k; P.big += ((s.big ? 1.18 : 1) - P.big) * k; P.br += ((s.br || 0) - P.br) * k;
    const moT = EAT.on ? EAT.mo : (s.mo ? s.mo + 0.1 * Math.sin(t * 5.5) : 0);
    P.mo += (moT - P.mo) * (1 - Math.exp(-dt * (EAT.on ? 28 : 11)));
    P.sv += (-190 * P.sq - 9 * P.sv) * dt; P.sq += P.sv * dt;
    P.jv += (-60 * P.jy - 7 * P.jv) * dt; P.jy += P.jv * dt;
    if (t > nb) { bt = t; nb = t + 2.5 + Math.random() * 3.5; }
    const kb = t - bt, blink = kb < 0.14 ? Math.sin((kb / 0.14) * Math.PI) : 0;
    const op = (s.half ? 0.6 : 1) * (1 - blink), breath = 1 + 0.014 * Math.sin(t * (cur === 'sleeping' ? 1.1 : 2.2));
    const sh = (s.shake ? Math.sin(t * 45) * 3 * Math.max(0, 1 - (t - ct)) : 0) + (s.trem ? Math.sin(t * 41) * 1.2 : 0);
    const bob = (s.bounce ? -Math.abs(Math.sin(t * 6)) * 4 : 0) + (s.hop ? -Math.abs(Math.sin(t * 4.2)) * 5 : 0) + P.jy;

    drawChar(s, k, dt, op, breath, sh, bob, true);
    if (FX[s.fx]) { c.save(); FX[s.fx](); c.restore(); c.globalAlpha = 1; }
    hooks.onAfterFrame?.(dt, now);

    const canRestNow = typeof hooks.canRest === 'function' ? hooks.canRest(now, { cur, base, eatOn: !!EAT.on, kb, P }) : false;
    if (canRestNow && now - lastAct > 2400 && kb > 0.16 && Math.abs(P.sv) < 0.05 && Math.abs(P.jv) < 0.05) {
      isResting = true;
      stopFrameLoop();
      avatars.stop();
      hooks.onRestChange?.(true);
      clearTimeout(restBlinkTimer);
      restBlinkTimer = setTimeout(() => {
        if (isResting && !hidden) {
          startFrameLoop();
          setTimeout(() => { if (isResting && !hidden) stopFrameLoop(); }, 300);
        }
      }, Math.max(7000, (nb - t) * 1000));
    }
  }

  function loopTick(now) {
    if (paused || hidden || !c) { frameRaf = null; return; }
    frameRaf = requestAnimationFrame(loopTick);
    if (now - lastPaint < frameGap(now) - 1) return;
    lastPaint = now;
    renderFrame(now);
  }

  function applyStateInternal(n) {
    wakeFromRest();
    cur = n;
    ct = t;
    const s = X[n] || X.idle;
    if (s.jump) P.jv = -150;
    if (s.squash) { P.sq = 0.34; P.sv = 0; }
    if (n === 'dizzy' || n === 'error') P.sv = -1;
    hooks.onStateChange?.(n, s, base);
  }

  // Mini avatares cacheados + animación de resorte para tarjetas e ítems de guardarropa
  const SNAP = new Map(), AVC = new Map(), AVB = new Map();
  function snap(color, outfitName, crop, blink = 0) {
    const key = color + '|' + outfitName + '|' + crop + '|' + blink;
    if (SNAP.has(key)) return SNAP.get(key);
    if (SNAP.size > 120) SNAP.delete(SNAP.keys().next().value);
    const off = document.createElement('canvas'); off.width = off.height = 400;
    const oc = off.getContext('2d'); oc.scale(2, 2); oc.translate(20, 20);
    const sv = { c, OUT, BODY, P: { ...P }, mx: ML.x, my: ML.y, ep: EP.map(a => a.slice()), tl: TAILS };
    c = oc; OUT = outfitName || 'none'; BODY = color; TAILS = false;
    Object.assign(P, { al: 0.5, ll: 0, ar: 0.5, lr: 0, lx: 0, ly: 0, tilt: 0, gi: 1, r: 59, g: 158, b: 255, sq: 0, sv: 0, jy: 0, jv: 0, sy: 1, big: 1, br: 0, bl: 0 });
    try { drawChar(X.idle, 1, 1, 1 - blink, 1, 0, 0, false); } finally {
      c = sv.c; OUT = sv.OUT; BODY = sv.BODY; TAILS = sv.tl; Object.assign(P, sv.P); ML.x = sv.mx; ML.y = sv.my; EP[0] = sv.ep[0]; EP[1] = sv.ep[1];
    }
    const [x0, y0, w, h] = crop === 'head' ? [-2, -20, 164, 156] : [-2, -20, 164, 166];
    const o2 = document.createElement('canvas'); o2.width = w * 2; o2.height = h * 2;
    o2.getContext('2d').drawImage(off, (x0 + 20) * 2, (y0 + 20) * 2, w * 2, h * 2, 0, 0, w * 2, h * 2);
    const u = o2.toDataURL(); SNAP.set(key, u); return u;
  }

  function avRender(color, outfitName, gx, gy, blinkVal, ex) {
    const off = document.createElement('canvas'); off.width = off.height = 400;
    const oc = off.getContext('2d'); oc.scale(2, 2); oc.translate(20, 20);
    const sv = { c, OUT, BODY, P: { ...P }, mx: ML.x, my: ML.y, ep: EP.map(a => a.slice()), tl: TAILS };
    const s = X[ex] || X.idle;
    c = oc; OUT = outfitName || 'none'; BODY = color; TAILS = false;
    Object.assign(P, { al: 0.5, ll: 0, ar: 0.5, lr: 0, lx: gx, ly: gy, tilt: gx * 0.1, gi: 1, r: 59, g: 158, b: 255, sq: 0, sv: 0, jy: 0, jv: 0, sy: 1, big: 1, br: 0, bl: s.blush ? 1 : 0, mo: 0 });
    try { drawChar(s, 1, 1, 1 - blinkVal, 1, 0, 0, false); } finally {
      c = sv.c; OUT = sv.OUT; BODY = sv.BODY; TAILS = sv.tl; Object.assign(P, sv.P); ML.x = sv.mx; ML.y = sv.my; EP[0] = sv.ep[0]; EP[1] = sv.ep[1];
    }
    return off;
  }

  function avBox(outfitName, color) {
    const k = (outfitName || 'none') + '|' + (color || '#fff');
    if (AVB.has(k)) return AVB.get(k);
    const d = avRender(color || '#fff', outfitName, 0, 0, 0, 'idle').getContext('2d').getImageData(0, 0, 400, 400).data;
    let x0 = 400, y0 = 400, x1 = -1, y1 = -1;
    for (let y = 0; y < 400; y++) for (let x = 0; x < 400; x++) if (d[(y * 400 + x) * 4 + 3] > 215) {
      if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y;
    }
    const b = x1 < x0 ? { x: 40, y: 40, w: 320, h: 320 } : { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 };
    AVB.set(k, b); return b;
  }

  function avFrame(color, outfitName, crop, gx, gy, blinkVal, ex, peek) {
    const key = [color, outfitName, gx, gy, blinkVal, ex].join('|');
    const hit = AVC.get(key); if (hit || peek) return hit || null;
    const b = avBox(outfitName, color), off = avRender(color, outfitName, gx, gy, blinkVal, ex);
    const sc = Math.min(66 / b.w, 66 / b.h), mg = 9 / sc, W = b.w * sc + 18, H = b.h * sc + 18;
    const o = document.createElement('canvas'); o.width = o.height = 84;
    const x = o.getContext('2d'); x.imageSmoothingQuality = 'high';
    x.drawImage(off, b.x - mg, b.y - mg, b.w + 2 * mg, b.h + 2 * mg, (84 - W) / 2, (84 - H) / 2, W, H);
    x.globalCompositeOperation = 'destination-in';
    const g = x.createRadialGradient(42, 42, 33, 42, 42, 43); g.addColorStop(0, '#000'); g.addColorStop(1, 'rgba(0,0,0,0)');
    x.fillStyle = g; x.fillRect(0, 0, 84, 84);
    if (AVC.size > 500) AVC.delete(AVC.keys().next().value);
    AVC.set(key, o); return o;
  }

  const avatars = (() => {
    const S = new WeakMap(), cl = (v, a, b) => Math.max(a, Math.min(b, v)), R = (a, b) => a + Math.random() * (b - a);
    const RM = global.matchMedia?.('(prefers-reduced-motion:reduce)').matches;
    let px = -1e4, py = -1e4, pt = 0, prevNow = performance.now(), loopRaf = null;
    global.addEventListener('pointermove', e => { px = e.clientX; py = e.clientY; pt = performance.now(); }, { passive: true });
    const bk = a => a < 0 ? 0 : a < 45 ? 0.55 : a < 120 ? 1 : a < 165 ? 0.55 : 0;
    function react(s, ex, ms, v) { s.ex = ex; s.exUntil = performance.now() + ms; if (v && !RM) s.vy = v; }
    function init(el) {
      const [col, out, crop, seed, mode] = (el.dataset.av || '#fff|none|head|0|lite').split('|');
      const avCv = el.firstElementChild, n = performance.now(), host = el.closest('button') || el, isWork = mode === 'working';
      const s = { el, cv: avCv, x: avCv.getContext('2d'), col, out, crop, lite: mode === 'lite', isWork, ph: (+seed || 0) * 1.9 + Math.random() * 6.28, gx: 0, gy: 0, tgx: 0, tgy: 0, gt: 0, ex: isWork ? 'working' : 'idle', exUntil: 0, nl: n + R(500, 2500), nb: n + R(700, 3800), bs: -1, dbl: 0, nh: n + R(2500, 7000), nw: n + R(9000, 22000), y: 0, vy: 0, r: 0, vr: 0, sx: 0, key: '' };
      host.addEventListener('pointerenter', () => react(s, 'happy', 950, -85));
      host.addEventListener('pointerdown', () => react(s, 'happy', 800, -120));
      S.set(el, s); return s;
    }
    function tick(now) {
      const dt = Math.min(0.05, Math.max(0.001, (now - prevNow) / 1000)); prevNow = now;
      if (document.hidden) return;
      let bud = 3;
      for (const el of document.querySelectorAll('.apw[data-av]')) {
        const s = S.get(el) || init(el), r = el.getBoundingClientRect(); if (!r.width) continue;
        const dx = px - (r.left + r.width / 2), dy = py - (r.top + r.height / 2), near = Math.hypot(dx, dy) < 200 && now - pt < 3500;
        if (!s.lite && !RM) {
          if (near) { s.tgx = cl(Math.round(dx / 36), -2, 2); s.tgy = cl(Math.round(dy / 28), -1, 1); s.nl = now + 700; }
          else if (now > s.nl) { s.tgx = Math.random() < 0.35 ? 0 : Math.round(R(-2, 2)); s.tgy = Math.random() < 0.65 ? 0 : Math.round(R(-1, 1)); s.nl = now + R(1100, 4200); }
          if (now - s.gt > 65) { s.gt = now; s.gx += Math.sign(s.tgx - s.gx); s.gy += Math.sign(s.tgy - s.gy); }
        }
        if (s.bs < 0 && now > s.nb) { s.bs = now; s.nb = now + R(1800, 5400); s.dbl = Math.random() < 0.2; }
        let lv = 0; if (s.bs >= 0) { const a = now - s.bs; lv = Math.max(bk(a), s.dbl ? bk(a - 300) : 0); if (a > 560) s.bs = -1; }
        if (s.isWork) { s.ex = 'working'; s.exUntil = now + 5000; }
        else if (s.ex !== 'idle' && now > s.exUntil) s.ex = 'idle';
        const gxv = s.gx * 0.4, gyv = s.gy * 0.45, key = gxv + '|' + gyv + '|' + lv + '|' + s.ex;
        if (key !== s.key) { const f = avFrame(s.col, s.out, s.crop, gxv, gyv, lv, s.ex, bud <= 0); if (f) { bud--; s.x.clearRect(0, 0, 84, 84); s.x.drawImage(f, 0, 0); s.key = key; } }
        if (!RM) {
          const trAngle = near ? cl(dx / 240, -1, 1) * 0.2 : Math.sin((now / 1000) * 0.8 + s.ph) * 0.03;
          s.vr += ((trAngle - s.r) * 140 - s.vr * 13) * dt; s.r += s.vr * dt;
          s.sx += ((near ? cl(dx / 70, -1, 1) * 1.5 : 0) - s.sx) * (1 - Math.exp(-dt * 9));
          s.vy += (-s.y * 420 - s.vy * 20) * dt; s.y += s.vy * dt;
          const sy = 1 + cl(-s.y / 46, -0.16, 0.16) + Math.sin((now / 1000) * 2.2 + s.ph) * 0.016, sxx = 1 / Math.sqrt(sy);
          s.cv.style.transform = `translate(${s.sx.toFixed(2)}px,${s.y.toFixed(2)}px) rotate(${s.r.toFixed(3)}rad) scale(${sxx.toFixed(3)},${sy.toFixed(3)})`;
        }
      }
    }
    function tickLoop(now) { if (paused || hidden) { loopRaf = null; return; } loopRaf = requestAnimationFrame(tickLoop); tick(now); }
    const start = () => { if (!loopRaf) loopRaf = requestAnimationFrame(tickLoop); };
    const stop = () => { if (loopRaf) { cancelAnimationFrame(loopRaf); loopRaf = null; } };
    return { start, stop, react };
  })();

  const MikuMascot = {
    STATES: X,
    getState: () => base,
    getCurState: () => cur,
    getConfig: () => ({ ...cfg }),
    applyConfig,
    mount(canvasEl, options = {}) {
      cv = canvasEl;
      hooks = options;
      if (options.config) applyConfig(options.config);
      c = cv.getContext('2d');
      c.setTransform(2, 0, 0, 2, 40, 40);
      hidden = false;
      paused = false;
      startFrameLoop();
      avatars.start();
      return MikuMascot;
    },
    setState(n) {
      if (!X[n]) return;
      if (n !== 'thinking') {
        if (emT) {
          clearTimeout(emT);
          emT = null;
        }
        LOCK.until = 0;
        LOCK.prio = 0;
        if (global.MikuUI?.runtime?.thk) {
          global.MikuUI.runtime.thk.on = 0;
          global.MikuUI.runtime.thk.end = Date.now();
        }
        hooks.onClearThinking?.();
      }
      if (hooks.isStateLocked?.(n, 0)) return;
      base = n;
      applyStateInternal(n);
    },
    emote(n, ms = 1800, prio = 1) {
      if (!X[n] || hooks.isStateLocked?.(n, prio)) return;
      if (n === 'look' || n.startsWith('look_')) return MikuMascot.look(n.split('_')[1] || 'left', ms);
      const now = Date.now();
      if (now < LOCK.until && prio < LOCK.prio) return;
      LOCK.until = now + ms;
      LOCK.prio = Math.max(prio || 1, HARD[n] || 0);
      applyStateInternal(n);
      clearTimeout(emT);
      emT = setTimeout(() => applyStateInternal(base), ms);
    },
    look(d = 'left', ms = 1800) {
      wakeFromRest();
      ov = { up: [0, -1], down: [0, 1], left: [-1, 0], right: [1, 0] }[d] || [0, 0];
      applyStateInternal('look');
      clearTimeout(emT);
      emT = setTimeout(() => applyStateInternal(base), ms);
    },
    lookAt(nx, ny) { mx = clamp(nx); my = clamp(ny); },
    setOutfit(outfitId, persist = true) {
      OUT = outfitId || 'none';
      if (persist) safeStorageSet('miku_outfit', OUT);
      wakeFromRest();
    },
    setColor(hexColor, eyeAccent) {
      BODY = hexColor || '#ffffff';
      if (eyeAccent) bl = eyeAccent;
      wakeFromRest();
    },
    setTwintails(enabled) {
      TAILS = !!enabled;
      safeStorageSet('miku_mascot_schema', '2');
      safeStorageSet('miku_tails', TAILS ? '1' : '0');
      wakeFromRest();
    },
    setEatProgress({ active, gx, gy, mouthOpen }) {
      EAT.on = active ? 1 : 0;
      if (gx != null) EAT.gx = gx;
      if (gy != null) EAT.gy = gy;
      if (mouthOpen != null) EAT.mo = mouthOpen;
      if (active) wakeFromRest();
    },
    impulse(sq = 0.26, jv = -120) {
      wakeFromRest();
      P.sq = sq; P.sv = 0; P.jv = jv;
    },
    getPhys: () => ({ ...P, base, cur, body: BODY, outfit: OUT, twintails: TAILS }),
    wake: () => { wakeFromRest(); if (base === 'sleeping') { base = 'idle'; MikuMascot.emote('surprised', 900); } },
    wakeFromRest,
    freeze: () => { hidden = true; stopFrameLoop(); avatars.stop(); },
    thaw: () => { hidden = false; paused = false; wakeFromRest(); startFrameLoop(); avatars.start(); },
    renderFrame,
    snap,
    avBox,
    avFrame,
    avtHtml: (col, out, i = 0, act = 0, crop = 'full', mode = '') => `<span class="apw ${crop}${act ? ' act' : ''}" data-av="${col}|${out}|${crop}|${i}|${mode}"><canvas class="apimg" width="84" height="84"></canvas></span>`,
    avatars
  };

  global.MikuMascot = MikuMascot;
  global.buddy = {
    setState: n => MikuMascot.setState(n),
    emote: (n, ms, prio) => MikuMascot.emote(n, ms, prio),
    look: (d, ms) => MikuMascot.look(d, ms),
    setOutfit: o => MikuMascot.setOutfit(o),
    setColor: h => MikuMascot.setColor(h),
    setTwintails: v => MikuMascot.setTwintails(v)
  };
})(window);
