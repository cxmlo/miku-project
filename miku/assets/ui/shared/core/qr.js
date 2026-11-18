/**
 * core/qr.js — Renderizador QR 100% offline para Miku-Link
 * Dibuja SVG del servidor (miku-gateway) cuando está conectado, o matriz
 * Reed-Solomon local en Canvas para vista previa y URI de emparejamiento.
 */
(function (global) {
  'use strict';

  const ECC = [10, 16, 26, 18, 24, 16, 18, 22, 22, 26];
  const BLK = [1, 1, 1, 2, 2, 4, 4, 4, 5, 5];

  const mul = (x, y) => {
    let z = 0;
    for (let i = 7; i >= 0; i--) { z = (z << 1) ^ ((z >>> 7) * 0x11D); z ^= ((y >>> i) & 1) * x; }
    return z;
  };
  const raw = v => {
    let r = (16 * v + 128) * v + 64;
    if (v >= 2) { const n = Math.floor(v / 7) + 2; r -= (25 * n - 10) * n - 55; if (v >= 7) r -= 36; }
    return r >> 3;
  };
  const cap = v => raw(v) - ECC[v - 1] * BLK[v - 1];
  const rsDiv = d => {
    const r = Array(d).fill(0); r[d - 1] = 1; let root = 1;
    for (let i = 0; i < d; i++) { for (let j = 0; j < d; j++) { r[j] = mul(r[j], root); if (j + 1 < d) r[j] ^= r[j + 1]; } root = mul(root, 2); }
    return r;
  };
  const rsRem = (data, div) => {
    const r = div.map(() => 0);
    for (const b of data) { const f = b ^ r.shift(); r.push(0); div.forEach((c, i) => { r[i] ^= mul(c, f); }); }
    return r;
  };
  const bit = (x, i) => ((x >>> i) & 1) !== 0;

  function make(text) {
    const bytes = [...new TextEncoder().encode(text)];
    let ver = 1;
    for (; ver <= 10; ver++) { if (4 + (ver < 10 ? 8 : 16) + bytes.length * 8 <= cap(ver) * 8) break; }
    if (ver > 10) throw new Error('QR: payload demasiado largo');
    const bits = [], put = (v, n) => { for (let i = n - 1; i >= 0; i--) bits.push((v >>> i) & 1); }, total = cap(ver) * 8;
    put(4, 4); put(bytes.length, ver < 10 ? 8 : 16); bytes.forEach(b => put(b, 8));
    put(0, Math.min(4, total - bits.length)); put(0, (8 - (bits.length % 8)) % 8);
    for (let p = 0xEC; bits.length < total; p ^= 0xEC ^ 0x11) put(p, 8);

    const data = [];
    for (let i = 0; i < bits.length; i += 8) { let b = 0; for (let j = 0; j < 8; j++) b = (b << 1) | bits[i + j]; data.push(b); }
    const nb = BLK[ver - 1], el = ECC[ver - 1], rc = raw(ver), ns = nb - (rc % nb), sl = Math.floor(rc / nb), div = rsDiv(el), blocks = [];
    for (let i = 0, k = 0; i < nb; i++) {
      const dat = data.slice(k, k + sl - el + (i < ns ? 0 : 1)); k += dat.length;
      const ecc = rsRem(dat, div); if (i < ns) dat.push(0); blocks.push(dat.concat(ecc));
    }
    const all = [];
    for (let i = 0; i < blocks[0].length; i++) blocks.forEach(b => { if (i !== sl - el || b.length > i) all.push(b[i]); });

    const n = ver * 4 + 17;
    const M = Array.from({ length: n }, () => Array(n).fill(false));
    const F = Array.from({ length: n }, () => Array(n).fill(false));
    const sf = (x, y, d) => { M[y][x] = d; F[y][x] = true; };

    for (let i = 0; i < n; i++) { sf(6, i, i % 2 === 0); sf(i, 6, i % 2 === 0); }
    const finder = (x, y) => { for (let dy = -4; dy <= 4; dy++) for (let dx = -4; dx <= 4; dx++) { const d = Math.max(Math.abs(dx), Math.abs(dy)), xx = x + dx, yy = y + dy; if (xx >= 0 && xx < n && yy >= 0 && yy < n) sf(xx, yy, d !== 2 && d !== 4); } };
    finder(3, 3); finder(n - 4, 3); finder(3, n - 4);
    const ap = [];
    if (ver > 1) { const na = Math.floor(ver / 7) + 2, st = Math.ceil((ver * 4 + 4) / (na * 2 - 2)) * 2; ap.push(6); for (let p = n - 7; ap.length < na; p -= st) ap.splice(1, 0, p); }
    ap.forEach((a, i) => ap.forEach((b, j) => {
      if (!((i === 0 && j === 0) || (i === 0 && j === ap.length - 1) || (i === ap.length - 1 && j === 0))) {
        for (let dy = -2; dy <= 2; dy++) for (let dx = -2; dx <= 2; dx++) sf(a + dx, b + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
      }
    }));

    const fmt = m => {
      let r = m; for (let i = 0; i < 10; i++) r = (r << 1) ^ ((r >>> 9) * 0x537);
      const b = ((m << 10) | r) ^ 0x5412;
      for (let i = 0; i <= 5; i++) sf(8, i, bit(b, i)); sf(8, 7, bit(b, 6)); sf(8, 8, bit(b, 7)); sf(7, 8, bit(b, 8));
      for (let i = 9; i < 15; i++) sf(14 - i, 8, bit(b, i));
      for (let i = 0; i < 8; i++) sf(n - 1 - i, 8, bit(b, i));
      for (let i = 8; i < 15; i++) sf(8, n - 15 + i, bit(b, i));
      sf(8, n - 8, true);
    };
    fmt(0);

    if (ver >= 7) {
      let r = ver; for (let i = 0; i < 12; i++) r = (r << 1) ^ ((r >>> 11) * 0x1F25);
      const b = (ver << 12) | r;
      for (let i = 0; i < 18; i++) { const v = bit(b, i), a = n - 11 + (i % 3), q = Math.floor(i / 3); sf(a, q, v); sf(q, a, v); }
    }

    for (let i = 0, right = n - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      for (let v = 0; v < n; v++) for (let j = 0; j < 2; j++) {
        const x = right - j, up = ((right + 1) & 2) === 0, y = up ? n - 1 - v : v;
        if (!F[y][x] && i < all.length * 8) { M[y][x] = bit(all[i >>> 3], 7 - (i & 7)); i++; }
      }
    }

    const apply = m => {
      for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) {
        let inv;
        switch (m) {
          case 0: inv = (x + y) % 2 === 0; break;
          case 1: inv = y % 2 === 0; break;
          case 2: inv = x % 3 === 0; break;
          case 3: inv = (x + y) % 3 === 0; break;
          case 4: inv = (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0; break;
          case 5: inv = (x * y) % 2 + (x * y) % 3 === 0; break;
          case 6: inv = ((x * y) % 2 + (x * y) % 3) % 2 === 0; break;
          default: inv = (((x + y) % 2) + ((x * y) % 3)) % 2 === 0;
        }
        if (!F[y][x] && inv) M[y][x] = !M[y][x];
      }
    };
    const pen = () => {
      let p = 0;
      for (let t = 0; t < 2; t++) for (let a = 0; a < n; a++) {
        let run = 1;
        for (let b = 1; b < n; b++) {
          const cur = t ? M[b][a] : M[a][b], prv = t ? M[b - 1][a] : M[a][b - 1];
          if (cur === prv) { run++; if (run === 5) p += 3; else if (run > 5) p++; } else run = 1;
        }
      }
      for (let y = 0; y < n - 1; y++) for (let x = 0; x < n - 1; x++) {
        const c = M[y][x]; if (c === M[y][x + 1] && c === M[y + 1][x] && c === M[y + 1][x + 1]) p += 3;
      }
      let dk = 0; M.forEach(r => r.forEach(v => { dk += v ? 1 : 0; }));
      return p + Math.floor(Math.abs(dk * 20 - n * n * 10) / (n * n)) * 10;
    };

    let best = 0, bp = 1e9;
    for (let m = 0; m < 8; m++) { apply(m); fmt(m); const p = pen(); apply(m); if (p < bp) { bp = p; best = m; } }
    apply(best); fmt(best);
    return M;
  }

  function draw(cv, text, px = 136, fg = '#111111', bg = '#ffffff') {
    const M = make(text), n = M.length, q = 4, s = Math.max(1, Math.floor(px / (n + 2 * q))), d = s * (n + 2 * q);
    cv.width = cv.height = d;
    cv.style.width = cv.style.height = px + 'px';
    const x = cv.getContext('2d');
    x.fillStyle = bg; x.fillRect(0, 0, d, d);
    x.fillStyle = fg;
    for (let y = 0; y < n; y++) for (let i = 0; i < n; i++) if (M[y][i]) x.fillRect((i + q) * s, (y + q) * s, s, s);
  }

  function renderPairingQr(cv, url, serverDataUrl) {
    if (!cv) return;
    if (serverDataUrl && serverDataUrl.startsWith('data:image/')) {
      const img = new Image();
      img.onload = () => {
        cv.width = cv.height = 288;
        cv.getContext('2d').drawImage(img, 0, 0, 288, 288);
      };
      img.src = serverDataUrl;
      return;
    }
    if (url) {
      try { draw(cv, url, 288); } catch (_) {}
    }
  }

  global.MikuQR = { make, draw, renderPairingQr };
})(window);
