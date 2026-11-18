/**
 * audio/sound-fx.js — Sintetizador Web Audio + Preload de Audios Personalizados
 * -----------------------------------------------------------------------------
 * - Desacoplado de CFG.voice: usa CFG.sfx_enabled y CFG.sfx_volume de forma independiente.
 * - Maneja AudioContext.resume() antes de programar nodos (evita silencio/glitches en el 1er clic).
 * - Usa DynamicsCompressorNode maestro + pre-carga de manifest.json sin falsos positivos.
 */
(function (global) {
  'use strict';

  let ctx = null;
  let masterCompressor = null;
  let masterGain = null;
  let duckTimer = null;
  let manifestSlots = {};
  const customEntries = new Map(); // slot -> { status: 'loading'|'ready'|'failed', audio: HTMLAudioElement|null }

  function ensureGraph() {
    if (!ctx) {
      const AudioCtx = global.AudioContext || global.webkitAudioContext;
      if (!AudioCtx) return null;
      ctx = new AudioCtx();
    }
    if (ctx && !masterGain) {
      masterGain = ctx.createGain();
      masterGain.gain.setValueAtTime(1, ctx.currentTime);
      if (typeof ctx.createDynamicsCompressor === 'function') {
        masterCompressor = ctx.createDynamicsCompressor();
        masterCompressor.threshold.setValueAtTime(-14, ctx.currentTime);
        masterCompressor.knee.setValueAtTime(12, ctx.currentTime);
        masterCompressor.ratio.setValueAtTime(6, ctx.currentTime);
        masterCompressor.attack.setValueAtTime(0.002, ctx.currentTime);
        masterCompressor.release.setValueAtTime(0.12, ctx.currentTime);
        masterCompressor.connect(masterGain);
        masterGain.connect(ctx.destination);
      } else {
        masterGain.connect(ctx.destination);
      }
    }
    return ctx;
  }

  function unlock() {
    const ac = ensureGraph();
    if (!ac) return Promise.resolve(null);
    if (ac.state === 'suspended' && typeof ac.resume === 'function') {
      return ac.resume().then(() => ac).catch(() => ac);
    }
    return Promise.resolve(ac);
  }

  function getOutputNode(ac) {
    return masterCompressor || masterGain || ac.destination;
  }

  function preloadCustomSlot(slot, fileName) {
    const clean = String(fileName || '').trim().replace(/^\.?\//, '');
    if (!clean) {
      customEntries.delete(slot);
      return;
    }
    const audio = new Audio(`/shared/audio/sounds/${clean}`);
    audio.preload = 'auto';
    const entry = { status: 'loading', audio };
    customEntries.set(slot, entry);
    audio.addEventListener('canplaythrough', () => {
      entry.status = 'ready';
    }, { once: true });
    audio.addEventListener('error', () => {
      entry.status = 'failed';
      entry.audio = null;
    }, { once: true });
    try { audio.load(); } catch (_) {}
  }

  async function loadManifest() {
    try {
      const res = await fetch('/shared/audio/sounds/manifest.json', { cache: 'no-store' });
      if (res.ok) {
        const data = await res.json();
        manifestSlots = data?.slots || {};
        customEntries.clear();
        for (const [slot, file] of Object.entries(manifestSlots)) {
          if (file) preloadCustomSlot(slot, file);
        }
      }
    } catch (_) {}
  }
  loadManifest();

  function getVolume() {
    try {
      const c = global.MikuUI?.CFG;
      if (c) {
        if (c.sfx_enabled === 0 || c.sfx_enabled === false) return 0;
        if (typeof c.sfx_volume === 'number') {
          return Math.max(0, Math.min(1, c.sfx_volume / 100));
        }
      }
    } catch (_) {}
    return 0.6;
  }

  function duckMedia(durationMs = 2000) {
    try {
      global.Bridge?.duckMedia?.(20);
      clearTimeout(duckTimer);
      duckTimer = setTimeout(() => {
        global.Bridge?.restoreMedia?.();
      }, durationMs);
    } catch (_) {}
  }

  function tryCustomSound(type, vol) {
    const entry = customEntries.get(type);
    if (!entry || entry.status !== 'ready' || !entry.audio) return false;
    try {
      const clip = typeof entry.audio.cloneNode === 'function' ? entry.audio.cloneNode(true) : entry.audio;
      clip.volume = vol;
      if (clip === entry.audio) clip.currentTime = 0;
      const p = clip.play();
      if (p && typeof p.catch === 'function') {
        p.catch(() => {
          const ac = ensureGraph();
          if (ac && ac.state === 'running') synthesize(ac, type, vol, {});
        });
      }
      return true;
    } catch (_) {
      return false;
    }
  }

  function synthTone(ac, dest, { type = 'sine', startFreq, endFreq, peakGain, attack = 0.003, duration = 0.03, offset = 0, filterFreq = null }) {
    const t = ac.currentTime + offset;
    const osc = ac.createOscillator();
    const gain = ac.createGain();
    osc.type = type;
    osc.frequency.setValueAtTime(Math.max(40, startFreq), t);
    if (endFreq && endFreq !== startFreq) {
      osc.frequency.exponentialRampToValueAtTime(Math.max(40, endFreq), t + Math.max(0.008, duration * 0.88));
    }
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.linearRampToValueAtTime(Math.max(0.0002, peakGain), t + attack);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + duration);

    if (filterFreq) {
      const filter = ac.createBiquadFilter();
      filter.type = 'bandpass';
      filter.frequency.setValueAtTime(filterFreq, t);
      filter.Q.setValueAtTime(2.2, t);
      osc.connect(filter);
      filter.connect(gain);
    } else {
      osc.connect(gain);
    }
    gain.connect(dest);
    osc.start(t);
    osc.stop(t + duration + 0.01);
  }

  function synthesize(ac, type, vol, opts = {}) {
    const dest = getOutputNode(ac);
    try {
      switch (type) {
        case 'click':
        case 'tap': {
          const p = opts.pitch || 820;
          synthTone(ac, dest, { startFreq: p, endFreq: p * 0.48, peakGain: 0.24 * vol, attack: 0.002, duration: 0.016, filterFreq: 1250 });
          break;
        }
        case 'pop':
        case 'open': {
          synthTone(ac, dest, { startFreq: 300, endFreq: 680, peakGain: 0.26 * vol, attack: 0.003, duration: 0.034 });
          break;
        }
        case 'close': {
          synthTone(ac, dest, { startFreq: 560, endFreq: 260, peakGain: 0.2 * vol, attack: 0.002, duration: 0.032 });
          break;
        }
        case 'toggle': {
          const on = opts.on !== false;
          synthTone(ac, dest, {
            type: 'triangle',
            startFreq: on ? 420 : 640,
            endFreq: on ? 780 : 350,
            peakGain: 0.19 * vol,
            attack: 0.003,
            duration: 0.028
          });
          break;
        }
        case 'tab': {
          synthTone(ac, dest, { startFreq: 560, endFreq: 410, peakGain: 0.18 * vol, attack: 0.002, duration: 0.024 });
          break;
        }
        case 'send': {
          synthTone(ac, dest, { startFreq: 350, endFreq: 980, peakGain: 0.19 * vol, attack: 0.004, duration: 0.056 });
          synthTone(ac, dest, { startFreq: 520, endFreq: 1420, peakGain: 0.13 * vol, attack: 0.004, duration: 0.056 });
          break;
        }
        case 'receive':
        case 'chime':
        case 'wake': {
          [523.25, 659.25].forEach((freq, i) => {
            synthTone(ac, dest, { startFreq: freq, peakGain: 0.18 * vol, attack: 0.008, duration: 0.17, offset: i * 0.07 });
          });
          break;
        }
        case 'notification':
        case 'alert':
        case 'update': {
          [587.33, 783.99, 987.77].forEach((freq, i) => {
            synthTone(ac, dest, { startFreq: freq, peakGain: 0.16 * vol, attack: 0.008, duration: 0.14, offset: i * 0.065 });
          });
          break;
        }
        case 'hold': {
          synthTone(ac, dest, { type: 'triangle', startFreq: 170, endFreq: 290, peakGain: 0.15 * vol, attack: 0.004, duration: 0.035 });
          break;
        }
        case 'error': {
          [260, 196].forEach((freq, i) => {
            synthTone(ac, dest, { type: 'sawtooth', startFreq: freq, endFreq: freq * 0.76, peakGain: 0.14 * vol, attack: 0.005, duration: 0.085, offset: i * 0.09 });
          });
          break;
        }
        default: {
          synthTone(ac, dest, { startFreq: 640, endFreq: 480, peakGain: 0.16 * vol, attack: 0.002, duration: 0.022 });
          break;
        }
      }
    } catch (_) {}
  }

  function play(type, opts = {}) {
    const vol = opts.force ? Math.max(0.25, getVolume()) : getVolume();
    if (vol <= 0.01) return;

    if (['notification', 'alert', 'error', 'chime', 'update'].includes(type)) {
      duckMedia(opts.duckMs || 2200);
    }

    if (tryCustomSound(type, vol)) return;
    const ac = ensureGraph();
    if (!ac) return;

    if (ac.state === 'running') {
      synthesize(ac, type, vol, opts);
    } else if (typeof ac.resume === 'function') {
      ac.resume().then(() => {
        if (ac.state === 'running') synthesize(ac, type, vol, opts);
      }).catch(() => {});
    }
  }

  const FEMALE_VOICE_HINTS = [
    'dalia', 'sabina', 'helena', 'paloma', 'laura', 'paulina', 'monica',
    'conchita', 'lucia', 'elvira', 'ximena', 'salome', 'camila', 'sofia',
    'zira', 'aria', 'jenny', 'emma', 'sonia', 'hazel', 'susan', 'female', 'mujer'
  ];

  function pickFemaleVoice(langPrefix) {
    if (!global.speechSynthesis?.getVoices) return null;
    const voices = global.speechSynthesis.getVoices() || [];
    const targetPrefix = String(langPrefix || 'es').toLowerCase().slice(0, 2);
    const inLang = voices.filter(v => String(v.lang || '').toLowerCase().startsWith(targetPrefix));
    const pool = inLang.length ? inLang : voices;
    const byFemaleHint = pool.find(v => {
      const name = `${v.name || ''} ${v.voiceURI || ''}`.toLowerCase();
      return FEMALE_VOICE_HINTS.some(h => name.includes(h));
    });
    return byFemaleHint || pool[0] || null;
  }

  function sanitizeTtsText(raw) {
    return String(raw || '')
      .replace(/```[\s\S]*?```/g, ' bloque de código ')
      .replace(/`([^`]+)`/g, '$1')
      .replace(/https?:\/\/\S+/g, ' enlace ')
      .replace(/[*_#>|]+/g, ' ')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 460);
  }

  let activeTtsAudio = null;

  function stopSpeaking() {
    try { global.speechSynthesis?.cancel?.(); } catch (_) {}
    if (activeTtsAudio) {
      try {
        activeTtsAudio.pause();
        activeTtsAudio.src = '';
      } catch (_) {}
      activeTtsAudio = null;
    }
  }

  function playBase64Audio(mime, b64, vol) {
    return new Promise((resolve, reject) => {
      try {
        stopSpeaking();
        const audio = new Audio(`data:${mime};base64,${b64}`);
        audio.volume = Math.max(0.1, Math.min(1, vol));
        activeTtsAudio = audio;
        audio.onended = () => {
          if (activeTtsAudio === audio) activeTtsAudio = null;
          resolve(true);
        };
        audio.onerror = () => {
          if (activeTtsAudio === audio) activeTtsAudio = null;
          reject(new Error('Audio decode failed'));
        };
        const p = audio.play();
        if (p && typeof p.catch === 'function') p.catch(reject);
      } catch (err) {
        reject(err);
      }
    });
  }

  function speakBrowserFallback(clean, cfg) {
    if (!global.speechSynthesis || !global.SpeechSynthesisUtterance) return false;
    try {
      global.speechSynthesis.cancel();
      const isEn = cfg.lang === 'en';
      const u = new global.SpeechSynthesisUtterance(clean);
      u.lang = isEn ? 'en-US' : 'es-MX';
      const female = pickFemaleVoice(isEn ? 'en' : 'es');
      if (female) u.voice = female;
      u.volume = Math.max(0.1, Math.min(1, (cfg.voice_volume ?? 80) / 100));
      u.pitch = Number(cfg.voice_pitch || 1.08);
      u.rate = Number(cfg.voice_rate || 1.02);
      global.speechSynthesis.speak(u);
      return true;
    } catch (_) {
      return false;
    }
  }

  function speakReply(rawText) {
    const cfg = global.MikuUI?.CFG || {};
    const ttsOn = cfg.voice_tts_enabled ?? cfg.voice ?? true;
    if (ttsOn === false || ttsOn === 0) return false;
    const clean = sanitizeTtsText(rawText);
    if (!clean) return false;
    const vol = Math.max(0.1, Math.min(1, (cfg.voice_volume ?? 80) / 100));
    const engine = cfg.voice_tts_engine || 'auto';
    const voice = cfg.voice_tts_voice || '';
    if (engine !== 'browser' && global.Bridge?.speakTts && global.MikuUI?.ST?.ws) {
      global.Bridge.speakTts(clean, engine, voice)
        .then(res => {
          if (res?.ok && res?.audio_b64 && res?.mime) {
            return playBase64Audio(res.mime, res.audio_b64, vol);
          }
          speakBrowserFallback(clean, cfg);
        })
        .catch(() => speakBrowserFallback(clean, cfg));
      return true;
    }
    return speakBrowserFallback(clean, cfg);
  }

  function previewVoiceTone() {
    const cfg = global.MikuUI?.CFG || {};
    const isEn = cfg.lang === 'en';
    const sample = isEn
      ? 'Hi, Miku neural female voice is ready.'
      : 'Hola, soy Miku y mi voz femenina está lista.';
    const prev = cfg.voice_tts_enabled;
    cfg.voice_tts_enabled = true;
    const ok = speakReply(sample);
    cfg.voice_tts_enabled = prev;
    if (ok) return true;
    play('chime', { force: true });
    return false;
  }

  document.addEventListener('pointerdown', e => {
    unlock();
    const el = e.target.closest('button, [data-a], [data-go], .tb, .sw, .sq, .miku-ag-pill, .rb-send, .crt-item button, .sw8, .mk-set-tab');
    if (!el || el.hasAttribute('data-test-sfx')) return;
    if (el.classList.contains('sw') || el.classList.contains('sq') || el.closest('.sw, .sq')) {
      const inp = el.querySelector('input') || el;
      play('toggle', { on: !inp.checked });
      return;
    }
    if (el.classList.contains('tb') || el.classList.contains('mk-set-tab') || el.dataset.go || el.dataset.a?.startsWith('go:')) {
      play('tab');
      return;
    }
    if (el.classList.contains('crt-pill') || el.classList.contains('miku-ag-pill')) {
      play('pop');
      return;
    }
    if (el.classList.contains('rb-send') || el.dataset.a === 'send') {
      play('send');
      return;
    }
    play('click');
  }, { capture: true, passive: true });

  document.addEventListener('keydown', () => unlock(), { capture: true, once: true, passive: true });

  global.SoundFX = {
    play,
    unlock,
    click: pitch => play('click', { pitch }),
    pop: () => play('pop'),
    open: () => play('open'),
    close: () => play('close'),
    toggle: on => play('toggle', { on }),
    tab: () => play('tab'),
    send: () => play('send'),
    receive: () => play('receive'),
    chime: () => play('chime'),
    alert: () => play('alert'),
    notification: () => play('notification'),
    hold: () => play('hold'),
    error: () => play('error'),
    previewVoice: previewVoiceTone,
    speakReply,
    stopSpeaking,
    pickFemaleVoice,
    duckMedia,
    reloadManifest: loadManifest,
    getAudioContext: ensureGraph,
    getStatus: () => ({
      state: ctx?.state || 'uninitialized',
      sfxEnabled: global.MikuUI?.CFG?.sfx_enabled !== 0 && global.MikuUI?.CFG?.sfx_enabled !== false,
      sfxVolume: global.MikuUI?.CFG?.sfx_volume ?? 60,
      customSlotsLoaded: Array.from(customEntries.entries()).filter(([, v]) => v.status === 'ready').map(([k]) => k)
    })
  };
})(window);
