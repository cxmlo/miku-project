/**
 * scenes/chat.js — Chat Multi-Agente, Renderizado Markdown Seguro y Menú Líquido (+)
 * ---------------------------------------------------------------------------------
 * Escapa estrictamente cualquier salida de agente antes de aplicar formato visual
 * para cumplir CSP 'self' y evitar inyección XSS / event handlers inline.
 */
(function (global) {
  'use strict';

  function formatAgentMarkdown(text) {
    const { esc } = global.MikuUI;
    let raw = String(text || '');
    const blocks = [];
    raw = raw.replace(/```(\w*)\n?([\s\S]*?)```/g, (_, lang, code) => {
      const idx = blocks.length;
      blocks.push(`<pre class="step-cmd-code"><code>${esc(code.trimEnd())}</code></pre>`);
      return `\u0000BLK${idx}\u0000`;
    });

    const lines = raw.split(/\r?\n/);
    const out = lines.map(line => {
      let s = esc(line);
      s = s.replace(/`([^`]+)`/g, '<code class="ag-code">$1</code>');
      s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
      const bulletMatch = s.match(/^\s*(?:[-*]|\u2022)\s+(.*)$/);
      if (bulletMatch) {
        return `<div class="ag-bullet"><span class="ag-bullet-dot">\u2022</span><span>${bulletMatch[1]}</span></div>`;
      }
      return s;
    }).join('<br>');

    return out.replace(/\u0000BLK(\d+)\u0000/g, (_, i) => blocks[Number(i)] || '');
  }

  function normalizeToolStep(st) {
    if (st && typeof st === 'object') return st;
    const raw = String(st || '').trim();
    const toolMatch = raw.match(/^Tool\s+(\S+)\s+->\s+(.+)$/);
    if (toolMatch) return { tool: toolMatch[1], status: toolMatch[2] };
    const colonIdx = raw.indexOf(':');
    if (colonIdx > 0) {
      return {
        tool: raw.slice(0, colonIdx).trim(),
        status: raw.slice(colonIdx + 1).trim() || 'ok'
      };
    }
    return { tool: raw || 'step', status: 'ok' };
  }

  function isSyntheticLocalStep(raw) {
    if (!raw) return true;
    const text = typeof raw === 'object' ? String(raw.tool || '') : String(raw);
    return text.startsWith('Router Local') || text.startsWith('Ahorro estimado');
  }

  function renderToolSteps(steps) {
    if (!Array.isArray(steps) || !steps.length) return '';
    const realSteps = steps.filter(s => !isSyntheticLocalStep(s));
    if (!realSteps.length) return '';
    const { esc } = global.MikuUI;
    const rows = realSteps.map(rawSt => {
      const st = normalizeToolStep(rawSt);
      const cmd = st.cmd ? `<div class="step-cmd-code">${esc(st.cmd)}</div>` : '';
      const out = st.out ? `<div class="step-out-preview">${esc(st.out)}</div>` : '';
      return `<div class="step-item"><div class="step-meta"><strong>${esc(st.tool || 'tool')}</strong><span>${esc(st.status || 'ok')}</span></div>${cmd}${out}</div>`;
    }).join('');
    return `<details class="agent-tools-box"><summary class="atb-summary"><span class="atb-label">Herramientas (${realSteps.length})</span><span>Ver</span></summary><div class="agent-steps-list">${rows}</div></details>`;
  }

  function renderThinkingBlock(reasoning, durSec) {
    if (!reasoning) return '';
    const { esc } = global.MikuUI;
    const durTag = durSec ? `<span>${Number(durSec).toFixed(1)}s</span>` : '';
    return `<details class="agent-thinking-box"><summary class="atb-summary"><span class="atb-label">Razonamiento</span>${durTag}</summary><div class="agent-steps-list"><div class="step-item"><div class="step-meta">${esc(reasoning)}</div></div></div></details>`;
  }

  function renderChatHeader() {
    const { curAg, getAgChat } = global.MikuUI;
    const { ic } = global.MikuComponents;
    const chat = getAgChat(curAg().id);
    const hasHistory = (Array.isArray(chat.history) && chat.history.length > 0) || Boolean(chat.lastQ || chat.lastA);
    const clearBtn = hasHistory
      ? `<button type="button" class="ib" data-chat-clear title="Nueva conversación" aria-label="Nueva conversación">${ic('retry')}</button>`
      : '';
    return global.MikuScenes.HD('', 'overview', clearBtn);
  }

  function renderChatCard() {
    const { ST, CFG, curAg, bodyOf, getAgChat, tr, esc } = global.MikuUI;
    const { ic } = global.MikuComponents;
    const ag = curAg();
    const acCol = bodyOf(ag);
    const chat = getAgChat(ag.id);

    let bodyHtml = '';
    const history = Array.isArray(chat.history) ? chat.history : [];

    if (!history.length && !chat.lastQ && !chat.lastA && !chat.busy) {
      const warn = ST.killSwitch ? '<span class="chat-warn-note">Kill Switch activo: solo lectura y respuesta local.</span>' : '';
      const apps = Array.isArray(ST.appsDetected) ? ST.appsDetected : [];
      const dynamicApp = apps.find(a => {
        const label = String(a?.name || a?.id || '').toLowerCase();
        return label && !label.includes('discord') && !label.includes('pinterest') && a?.installed !== false;
      });
      const dynamicChip = dynamicApp
        ? `<button type="button" class="chat-qchip" data-quick-prompt="Abrir ${esc(dynamicApp.name || dynamicApp.id)}">Abrir ${esc(dynamicApp.name || dynamicApp.id)}</button>`
        : '';
      bodyHtml = `<div class="chat-welcome">
        <div class="chat-welcome-head">
          <span class="chat-welcome-title">¿En qué te ayudo hoy?</span>
          <span class="chat-welcome-sub">${esc(ST.ws ? 'Core conectado · Modo ahorro activo' : 'Modo local')}</span>
        </div>
        ${warn}
        <div class="chat-qchips">
          <button type="button" class="chat-qchip" data-quick-prompt="Analizar pantalla">Analizar pantalla</button>
          <button type="button" class="chat-qchip" data-quick-prompt="Estado del equipo">Estado del equipo</button>
          <button type="button" class="chat-qchip" data-quick-prompt="Resumir portapapeles">Resumir portapapeles</button>
          <button type="button" class="chat-qchip" data-quick-prompt="Abrir navegador">Abrir navegador</button>
          ${dynamicChip}
        </div>
      </div>`;
    } else {
      const turns = history.length ? history : [
        ...(chat.lastQ ? [{ role: 'user', text: chat.lastQ }] : []),
        ...(chat.lastA ? [{ role: 'agent', text: chat.lastA, steps: chat.steps, reasoning: chat.reasoning, thinkDur: chat.thinkDur }] : [])
      ];

      bodyHtml = turns.map(t => {
        if (t.role === 'user') {
          return `<div class="msg-user-wrap"><div class="msg-user-bubble" style="--pc:${acCol}"><div class="msg-user-text">${esc(t.text)}</div></div></div>`;
        }
        const isErr = Boolean(t.error);
        const saverBadge = (t.localRoute || t.local_route)
          ? '<span class="msg-badge-saver" title="Resuelto localmente sin gasto de API">0 tokens</span>'
          : '';
        const errActions = isErr
          ? `<div class="msg-err-actions">
              ${t.retryPrompt ? `<button type="button" class="msg-err-btn" data-retry-prompt="${esc(t.retryPrompt)}">Reintentar</button>` : ''}
              <button type="button" class="msg-err-btn sec" data-open-ext-settings="providers">Revisar API Keys</button>
            </div>`
          : '';
        return `<div class="msg-agent-col">
          <div class="msg-agent-bubble${isErr ? ' is-error' : ''}" style="--pc:${acCol}">
            <div class="msg-agent-meta">
              <span class="msg-agent-badge"><i class="msg-agent-badge-dot"></i>${esc(ag.name)}${saverBadge}</span>
              <span class="msg-agent-time">${esc(t.time || 'Ahora')}</span>
            </div>
            ${renderThinkingBlock(t.reasoning, t.thinkDur)}
            ${renderToolSteps(t.steps)}
            <div class="msg-agent-inbox"><div class="msg-agent-text">${formatAgentMarkdown(t.text)}</div></div>
            ${errActions}
          </div>
        </div>`;
      }).join('');

      if (chat.busy) {
        bodyHtml += `<div class="msg-agent-col">
          <div class="msg-agent-bubble is-busy" style="--pc:${acCol}">
            <div class="msg-agent-meta"><span class="msg-agent-badge"><i class="msg-agent-badge-dot"></i>${esc(ag.name)}</span><span class="msg-agent-time">${esc(tr('thinking'))}</span></div>
            <div class="chat-typing-dots"><span></span><span></span><span></span></div>
          </div>
        </div>`;
      }
    }

    const att = chat.lastFile;
    const attHtml = att
      ? `<div id="chat-att-wrap"><div class="chat-attached-pill" title="${esc(att.name)}">
          ${ic('file')}
          <span class="caf-name">${esc(att.name)}</span>
          <span class="caf-size">${att.size ? Math.max(1, Math.round(att.size / 1024)) + ' KB' : ''}</span>
          <button type="button" class="caf-send" data-att-send title="Analizar con agente">${ic('send')}</button>
          <button type="button" class="caf-remove" data-att-remove title="Quitar archivo">${ic('x')}</button>
        </div></div>`
      : '';

    const rt = global.MikuUI?.runtime || {};
    const voiceConsentHtml = rt.voiceConsentOpen
      ? `<div class="voice-consent-card" role="dialog" aria-label="Configurar voz Opt-In">
          <div class="voice-consent-head">
            <strong>Activar dictado por voz (Opt-In)</strong>
            <button type="button" class="voice-consent-close" data-voice-consent="dismiss" aria-label="Cerrar">${ic('x')}</button>
          </div>
          <p class="voice-consent-sub">El micrófono permanece bloqueado hasta que elijas un motor:</p>
          <div class="voice-consent-opts">
            <button type="button" class="voice-consent-btn sec" data-voice-consent="off">No usar voz</button>
            <button type="button" class="voice-consent-btn prim" data-voice-consent="browser_webspeech">Activar Voz Rápida estilo Miku — Gratis</button>
            <button type="button" class="voice-consent-btn" data-voice-consent="groq_whisper">Usar Groq Whisper PTT</button>
          </div>
        </div>`
      : '';

    const sendBtn = chat.busy
      ? `<button type="button" id="q-stop-btn" class="rb-send" data-chat-stop title="Detener">${ic('pause')}</button>`
      : `<button type="button" id="q-send-btn" class="rb-send" data-chat-send title="Enviar">${ic('send')}</button>`;

    return `<div class="card mk chat-card" style="--pc:${acCol}">
      <div class="chat-mascot-side">
        <span class="cs" data-w="64" data-h="64" style="width:64px;height:64px"></span>
        <div class="chat-agent-name">${esc(ag.name)}</div>
        <span class="chat-agent-role">${esc(ag.id === 'miku' ? 'Orquestadora' : tr('subagent'))}</span>
      </div>
      <div class="chat-main-side">
        <div class="chat-convo-body scr" id="chat-scroll">${bodyHtml}</div>
        ${voiceConsentHtml}
        ${attHtml}
        <div class="fld">
          <div class="crt-stage">
            <button type="button" id="crt-pill" class="crt-pill" aria-expanded="false" title="Acciones rápidas">${ic('plus')}</button>
            <div id="crt-panel" class="crt-panel">
              <ul class="crt-menu">
                <li class="crt-item"><button type="button" data-crt="capture-screen">${ic('screen')}<span>Analizar pantalla</span></button></li>
                <li class="crt-item"><button type="button" data-crt="attach-file">${ic('file')}<span>Adjuntar archivo</span></button></li>
                <li class="crt-item"><button type="button" data-crt="capture-clipboard">${ic('copy')}<span>Pegar portapapeles</span></button></li>
                <li class="crt-item"><button type="button" data-crt="open-wardrobe">${ic('shirt')}<span>Estilo y Guardarropa</span></button></li>
                <li class="crt-item"><button type="button" data-crt="open-link">${ic('phone')}<span>Miku-Link (QR)</span></button></li>
                <li class="crt-item"><button type="button" data-crt="open-settings">${ic('gear')}<span>Gatekeeper y Ajustes</span></button></li>
              </ul>
            </div>
          </div>
          <textarea id="qi" rows="1" autocomplete="off" spellcheck="true" placeholder="${esc(tr('chat_placeholder'))}">${esc(chat.draft || '')}</textarea>
          <button type="button" class="ib" data-a="toggle-mic" title="Dictado por voz">${ic('mic')}</button>
          ${sendBtn}
        </div>
      </div>
    </div>`;
  }

  function resizeComposer(el, onResize) {
    if (!el) return;
    const prev = el.style.height;
    el.style.height = 'auto';
    const nextH = Math.min(108, Math.max(30, el.scrollHeight));
    el.style.height = nextH + 'px';
    if (prev !== el.style.height) onResize?.();
  }

  function submitChatPrompt(hooks, overrideText = null) {
    const { $, curAg, getAgChat, saveAgChats, runtime } = global.MikuUI;
    const inp = $('#qi');
    const txt = overrideText != null ? String(overrideText).trim() : (inp ? inp.value.trim() : '');
    const ag = curAg();
    const chat = getAgChat(ag.id);
    if (chat.busy && (Date.now() - (chat.thinkStart || 0)) < 30000) return;
    if (!txt && !chat.lastFile) return;

    const promptText = txt || `Analiza el archivo adjunto: ${chat.lastFile?.name || ''}`;
    chat.draft = '';
    runtime.editingDraft = false;
    runtime.refocusChatInput = true;
    if (inp && overrideText == null) {
      inp.value = '';
      inp.style.height = 'auto';
    }

    chat.lastQ = promptText;
    chat.busy = true;
    chat.unread = false;
    chat.thinkStart = Date.now();
    runtime.thk.on = 1;
    runtime.thk.start = chat.thinkStart;
    runtime.thk.end = 0;
    runtime.thk.dur = 0;

    if (runtime.turnWatchdogTimer) {
      clearTimeout(runtime.turnWatchdogTimer);
      runtime.turnWatchdogTimer = null;
    }
    runtime.turnWatchdogTimer = setTimeout(() => {
      global.MikuUI?.finalizeTurn?.({ reason: 'watchdog_timeout' });
    }, 30000);

    if (!Array.isArray(chat.history)) chat.history = [];
    chat.history.push({ role: 'user', text: promptText, time: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) });
    saveAgChats();

    global.SoundFX?.send?.();
    global.MikuMascot?.setState?.('thinking');
    hooks?.rerender?.();
    global.Bridge?.sendChat?.(promptText, ag, chat.lastFile || null);
  }

  let _chatAbort = null;

  function bindChatEvents(col, hooks) {
    if (_chatAbort) _chatAbort.abort();
    _chatAbort = new AbortController();
    const { signal } = _chatAbort;
    const rawHooks = hooks;
    hooks = {
      ...rawHooks,
      rerender: () => {
        if (!signal.aborted) rawHooks?.rerender?.();
      }
    };

    const { $, curAg, getAgChat, runtime } = global.MikuUI;
    const scrollEl = $('#chat-scroll');
    if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;

    const inp = $('#qi');
    if (inp) {
      resizeComposer(inp);
      const activateComposer = () => {
        runtime.editingDraft = true;
        global.Bridge?.windowControl?.('activate_input');
      };
      inp.addEventListener('pointerdown', activateComposer, { signal });
      inp.addEventListener('focus', activateComposer, { signal });
      inp.addEventListener('blur', () => {
        runtime.editingDraft = false;
      }, { signal });
      inp.addEventListener('input', () => {
        getAgChat(curAg().id).draft = inp.value;
        resizeComposer(inp, () => hooks?.fit?.());
      }, { signal });
      inp.addEventListener('keydown', e => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault();
          submitChatPrompt(hooks);
        }
      }, { signal });

      if (runtime.refocusChatInput) {
        runtime.refocusChatInput = false;
        requestAnimationFrame(() => {
          try {
            global.Bridge?.windowControl?.('activate_input');
            inp.focus({ preventScroll: true });
            const end = inp.value.length;
            inp.setSelectionRange?.(end, end);
          } catch (_) {}
        });
      }
    }

    const crtBtn = $('#crt-pill');
    const crtPanel = $('#crt-panel');
    if (crtBtn && crtPanel) {
      crtBtn.addEventListener('click', e => {
        e.stopPropagation();
        const open = crtPanel.classList.toggle('open');
        crtBtn.classList.toggle('open', open);
        crtBtn.setAttribute('aria-expanded', String(open));
        global.SoundFX?.pop?.();
        requestAnimationFrame(() => hooks?.fit?.());
      });
    }

    const onCol = (type, fn) => col.addEventListener(type, fn, { signal });
    onCol('click', async e => {
      const goSet = e.target.closest('[data-go-settings]');
      if (goSet) {
        runtime.settingsTab = goSet.dataset.goSettings || 'modules';
        hooks?.go?.('settings');
        return;
      }

      if (e.target.closest('[data-chat-send]')) {
        submitChatPrompt(hooks);
        return;
      }

      const retryBtn = e.target.closest('[data-retry-prompt]');
      if (retryBtn) {
        const retryTxt = retryBtn.dataset.retryPrompt || '';
        if (retryTxt) submitChatPrompt(hooks, retryTxt);
        return;
      }

      const extSetBtn = e.target.closest('[data-open-ext-settings]');
      if (extSetBtn) {
        global.Bridge?.openSettings?.(extSetBtn.dataset.openExtSettings || 'providers');
        return;
      }

      const qp = e.target.closest('[data-quick-prompt]');
      if (qp) {
        const promptText = qp.dataset.quickPrompt || '';
        const box = $('#qi');
        if (box && promptText) {
          box.value = promptText;
          getAgChat(curAg().id).draft = promptText;
          submitChatPrompt(hooks);
        }
        return;
      }

      if (e.target.closest('[data-chat-stop]')) {
        global.Bridge?.cancel?.();
        if (typeof global.MikuUI?.finalizeTurn === 'function') {
          global.MikuUI.finalizeTurn({ reason: 'cancel' });
        } else {
          const ag = curAg();
          getAgChat(ag.id).busy = false;
          saveAgChats();
          global.MikuMascot?.setState?.('idle');
          hooks?.rerender?.();
        }
        return;
      }

      if (e.target.closest('[data-chat-clear]')) {
        const ag = curAg();
        const c = getAgChat(ag.id);
        c.history = []; c.lastQ = ''; c.lastA = ''; c.busy = false; c.steps = [];
        saveAgChats();
        global.SoundFX?.click?.();
        hooks?.rerender?.();
        return;
      }

      if (e.target.closest('[data-att-remove]')) {
        const ag = curAg();
        getAgChat(ag.id).lastFile = null;
        saveAgChats();
        hooks?.rerender?.();
        return;
      }

      if (e.target.closest('[data-att-send]')) {
        submitChatPrompt(hooks);
        return;
      }

      const vcBtn = e.target.closest('[data-voice-consent]');
      if (vcBtn) {
        const choice = vcBtn.dataset.voiceConsent;
        if (choice === 'dismiss') {
          if (global.MikuVoiceRecognizer?.closeConsent) global.MikuVoiceRecognizer.closeConsent();
          else if (global.MikuUI?.runtime) global.MikuUI.runtime.voiceConsentOpen = false;
        } else {
          global.MikuVoiceRecognizer?.applyConsentChoice?.(choice);
        }
        hooks?.rerender?.();
        return;
      }

      const crtAct = e.target.closest('[data-crt]');
      if (crtAct) {
        const act = crtAct.dataset.crt;
        crtPanel?.classList.remove('open');
        crtBtn?.classList.remove('open');
        if (act === 'capture-screen') {
          submitChatPrompt(hooks, 'Analizar pantalla');
        } else if (act === 'attach-file') {
          document.getElementById('miku-file-input')?.click();
        } else if (act === 'capture-clipboard') {
          try {
            const clip = await navigator.clipboard.readText();
            const cur = $('#qi');
            if (cur && clip) {
              cur.value = (cur.value ? cur.value + ' ' : '') + clip.slice(0, 600);
              getAgChat(curAg().id).draft = cur.value;
              resizeComposer(cur, () => hooks?.fit?.());
            }
          } catch (_) {}
          global.Bridge?.windowControl?.('activate_input');
          $('#qi')?.focus();
        } else if (act === 'open-wardrobe') {
          hooks?.go?.('wardrobe');
        } else if (act === 'open-link') {
          hooks?.go?.('link');
        } else if (act === 'open-settings') {
          hooks?.go?.('settings');
        }
        return;
      }

      const zoomEl = e.target.closest('[data-zoom]');
      if (zoomEl) {
        const src = zoomEl.getAttribute('data-zoom');
        if (!src) return;
        const modal = document.createElement('div');
        modal.className = 'zoom-modal-backdrop';
        const img = document.createElement('img');
        img.src = src;
        modal.appendChild(img);
        modal.addEventListener('click', () => modal.remove());
        document.body.appendChild(modal);
        return;
      }

      if (crtPanel?.classList.contains('open') && !e.target.closest('.crt-stage')) {
        crtPanel.classList.remove('open');
        crtBtn?.classList.remove('open');
        requestAnimationFrame(() => hooks?.fit?.());
      }
    });
  }

  global.MikuChat = {
    formatAgentMarkdown,
    renderChatHeader,
    renderChatCard,
    submitChatPrompt,
    bindChatEvents
  };
  if (global.MikuUI) {
    global.MikuUI.submitChatPrompt = submitChatPrompt;
  }
})(window);
