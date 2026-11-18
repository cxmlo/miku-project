(function (global) {
  'use strict';

  const SpeechCtor = global.SpeechRecognition || global.webkitSpeechRecognition || null;
  let activeRecognition = null;
  let activeEngine = 'off';
  let active = false;
  let lastFinalText = '';

  function getCfg() {
    return global.MikuUI?.CFG || {};
  }

  function isEnabled() {
    const cfg = getCfg();
    const engine = String(cfg.voice_stt_engine || 'off');
    return Boolean(cfg.voice_stt_enabled) && engine !== 'off';
  }

  function getEngine() {
    const cfg = getCfg();
    if (!cfg.voice_stt_enabled) return 'off';
    const raw = String(cfg.voice_stt_engine || 'browser_webspeech');
    return raw === 'off' ? 'browser_webspeech' : raw;
  }

  function setMicActive(on, engine) {
    active = Boolean(on);
    if (engine) activeEngine = engine;
    const st = global.MikuUI?.ST;
    if (st) st.mic = active ? 1 : 0;
    if (!active && !st?.busy) {
      global.MikuMascot?.setState?.('idle');
    } else if (active) {
      global.SoundFX?.stopSpeaking?.();
      global.MikuMascot?.wakeFromRest?.();
      global.MikuMascot?.setState?.('listening');
    }
  }

  function deliverTranscript(text, isFinal, engine) {
    const clean = String(text || '').trim();
    if (!clean) return;
    if (isFinal && clean === lastFinalText) return;
    if (isFinal) lastFinalText = clean;

    const handler = global.MikuUI?.onVoiceTranscript;
    if (typeof handler === 'function') {
      handler({ text: clean, isFinal: Boolean(isFinal), engine: engine || activeEngine });
    }
  }

  function stopWebSpeech() {
    if (!activeRecognition) return;
    const rec = activeRecognition;
    activeRecognition = null;
    try {
      rec.onresult = null;
      rec.onerror = null;
      rec.onend = null;
      rec.stop();
    } catch (_) {}
  }

  function stop() {
    const wasActive = active || Boolean(activeRecognition);
    stopWebSpeech();
    if (wasActive && (activeEngine === 'local_sapi' || activeEngine === 'groq_whisper')) {
      global.Bridge?.dictate?.(false, activeEngine);
    }
    setMicActive(false, 'off');
  }

  function fallbackToNative(reason) {
    stopWebSpeech();
    const cfg = getCfg();
    const hasGroq = Boolean(cfg.groq_api_key || cfg.has_groq_key);
    const fallbackEngine = hasGroq ? 'groq_whisper' : 'local_sapi';
    if (global.MikuUI?.setSetting) {
      global.MikuUI.setSetting('voice_stt_engine', fallbackEngine);
    } else if (global.MikuUI?.CFG) {
      global.MikuUI.CFG.voice_stt_engine = fallbackEngine;
      global.Bridge?.setSetting?.('voice_stt_engine', fallbackEngine);
    }
    activeEngine = fallbackEngine;
    setMicActive(true, fallbackEngine);
    global.MikuUI?.onVoiceStatus?.({
      status: 'fallback',
      from: 'browser_webspeech',
      to: fallbackEngine,
      reason: String(reason || 'unavailable')
    });
    global.Bridge?.dictate?.(true, fallbackEngine);
    return { started: true, engine: fallbackEngine, fallback: true };
  }

  function startBrowserWebSpeech() {
    if (!SpeechCtor) {
      return fallbackToNative('webspeech-missing');
    }
    stopWebSpeech();
    lastFinalText = '';
    const rec = new SpeechCtor();
    const locale = String(getCfg().language || global.navigator?.language || 'es-ES');
    rec.lang = locale.startsWith('en') ? 'en-US' : 'es-ES';
    rec.interimResults = true;
    rec.continuous = false;
    rec.maxAlternatives = 1;

    rec.onresult = (ev) => {
      let interim = '';
      let finalChunk = '';
      for (let i = ev.resultIndex || 0; i < ev.results.length; i += 1) {
        const res = ev.results[i];
        const txt = res?.[0]?.transcript || '';
        if (res.isFinal) finalChunk += txt;
        else interim += txt;
      }
      if (finalChunk.trim()) {
        deliverTranscript(finalChunk, true, 'browser_webspeech');
      } else if (interim.trim()) {
        deliverTranscript(interim, false, 'browser_webspeech');
      }
    };

    rec.onerror = (ev) => {
      const code = String(ev?.error || 'unknown');
      if (code === 'network' || code === 'service-not-allowed' || code === 'not-allowed') {
        fallbackToNative(code);
        return;
      }
      if (code !== 'aborted' && code !== 'no-speech') {
        global.MikuUI?.onVoiceStatus?.({ status: 'error', engine: 'browser_webspeech', reason: code });
      }
      stop();
    };

    rec.onend = () => {
      if (activeRecognition === rec) {
        activeRecognition = null;
        if (active && activeEngine === 'browser_webspeech') {
          setMicActive(false, 'off');
        }
      }
    };

    activeRecognition = rec;
    setMicActive(true, 'browser_webspeech');
    try {
      rec.start();
      return { started: true, engine: 'browser_webspeech' };
    } catch (err) {
      return fallbackToNative(err?.message || 'start-failed');
    }
  }

  function start() {
    if (!isEnabled()) {
      openConsent();
      return { started: false, consentRequired: true };
    }
    const engine = getEngine();
    lastFinalText = '';
    if (engine === 'browser_webspeech') {
      return startBrowserWebSpeech();
    }
    setMicActive(true, engine);
    global.Bridge?.dictate?.(true, engine);
    return { started: true, engine };
  }

  function toggle() {
    if (!isEnabled()) {
      openConsent();
      return { started: false, consentRequired: true };
    }
    if (active) {
      stop();
      return { started: false };
    }
    return start();
  }

  function openConsent() {
    const rt = global.MikuUI?.runtime;
    if (rt) rt.voiceConsentOpen = true;
    global.MikuUI?.onVoiceConsentOpen?.(true);
    return true;
  }

  function closeConsent() {
    const rt = global.MikuUI?.runtime;
    if (rt) rt.voiceConsentOpen = false;
    global.MikuUI?.onVoiceConsentOpen?.(false);
  }

  function applyConsentChoice(choice) {
    const mode = String(choice || 'off');
    closeConsent();
    if (mode === 'off') {
      global.Bridge?.saveSettings?.({ voice_stt_enabled: false, voice_stt_engine: 'off' });
      if (global.MikuUI?.CFG) {
        global.MikuUI.CFG.voice_stt_enabled = false;
        global.MikuUI.CFG.voice_stt_engine = 'off';
      }
      stop();
      return { enabled: false, engine: 'off' };
    }
    const engine = mode === 'groq_whisper' ? 'groq_whisper' : (mode === 'local_sapi' ? 'local_sapi' : 'browser_webspeech');
    if (global.MikuUI?.CFG) {
      global.MikuUI.CFG.voice_stt_enabled = true;
      global.MikuUI.CFG.voice_stt_engine = engine;
    }
    global.Bridge?.saveSettings?.({ voice_stt_enabled: true, voice_stt_engine: engine });
    return start();
  }

  let wakeRec = null;
  let wakeActive = false;
  const WAKE_WORD_RE = /\b(oye\s+)?miku\b|hey\s+miku/i;

  function stopWakeWord() {
    wakeActive = false;
    if (!wakeRec) return;
    const r = wakeRec;
    wakeRec = null;
    try {
      r.onresult = null;
      r.onerror = null;
      r.onend = null;
      r.stop();
    } catch (_) {}
  }

  function startWakeWord() {
    const cfg = getCfg();
    if (!SpeechCtor || !isEnabled() || !cfg.voice_wake_word_enabled || active) {
      return false;
    }
    if (wakeRec) return true;
    const rec = new SpeechCtor();
    const locale = String(cfg.language || global.navigator?.language || 'es-ES');
    rec.lang = locale.startsWith('en') ? 'en-US' : 'es-ES';
    rec.interimResults = true;
    rec.continuous = true;
    rec.maxAlternatives = 1;
    rec.onresult = (ev) => {
      for (let i = ev.resultIndex || 0; i < ev.results.length; i += 1) {
        const txt = String(ev.results[i]?.[0]?.transcript || '');
        if (WAKE_WORD_RE.test(txt)) {
          stopWakeWord();
          global.SoundFX?.play?.('wake');
          global.MikuMascot?.wakeFromRest?.();
          start();
          return;
        }
      }
    };
    rec.onerror = (ev) => {
      const code = String(ev?.error || '');
      if (code === 'not-allowed' || code === 'service-not-allowed' || code === 'network') {
        stopWakeWord();
      }
    };
    rec.onend = () => {
      const stillWanted = Boolean(getCfg().voice_wake_word_enabled) && isEnabled() && !active;
      wakeRec = null;
      if (wakeActive && stillWanted) {
        setTimeout(() => startWakeWord(), 400);
      }
    };
    wakeRec = rec;
    wakeActive = true;
    try {
      rec.start();
      return true;
    } catch (_) {
      stopWakeWord();
      return false;
    }
  }

  global.MikuVoiceRecognizer = {
    isEnabled,
    getEngine,
    isActive: () => active,
    isWakeWordActive: () => wakeActive,
    start,
    stop,
    toggle,
    startWakeWord,
    stopWakeWord,
    openConsent,
    closeConsent,
    applyConsentChoice,
    handleNativeTranscript: (text, isFinal, engine) => deliverTranscript(text, isFinal, engine || activeEngine),
  };
})(window);
