/**
 * app.js — Orquestador de Dynamic Island (Miku Core UI)
 * ----------------------------------------------------------------------
 * Conecta MikuMascot + Bridge + SoundFX + MikuUI + MikuScenes sin código
 * bloqueante ni llamadas de red externas. 100% compatible con CSP 'self'.
 */
(function (global) {
  'use strict';

  const { $, ST, CFG, MUS, PRIN, OL, runtime, curAg, getAgChat, saveAgChats, saveAgents, saveCfg, bodyOf, usedByColor, setSela, getUnreadAgent, loadLocale } = global.MikuUI;

  const hostParams = new URLSearchParams(global.location?.search || '');
  const isWebViewHost = !!global.chrome?.webview || hostParams.get('core') === '1' || hostParams.get('embedded') === '1';
  if (!isWebViewHost) document.documentElement.classList.add('standalone-stage');
  if (hostParams.get('preview') === '1') {
    document.documentElement.classList.add('preview-mode');
    const previewDock = document.querySelector('.preview-dock');
    if (previewDock) previewDock.removeAttribute('hidden');
  }

  const isl = $('#isl');
  const innerEl = $('#in');
  const colEl = $('#col');
  const mkCanvas = $('#mk');
  const fileInput = $('#miku-file-input');

  let LOCKI = 0;
  let holdTimer = 0;
  let closingTimer = 0;
  let wBackScene = 'overview';
  let slotPos = { x: 24, y: 12, s: 0.16, ready: false };

  function syncMascotToAgent(ag = curAg()) {
    if (!ag) return;
    const isPrin = ag === PRIN || ag.id === 'miku';
    const bc = isPrin ? (runtime.mascotConfig?.defaultColor || '#ffffff') : bodyOf(ag);
    global.MikuMascot?.setColor?.(bc);
    global.MikuMascot?.setOutfit?.(ag.outfit || 'none', isPrin);
  }

  function syncDockActive() {
    document.querySelectorAll('.preview-dock button[data-go]').forEach(b => {
      b.classList.toggle('p', b.dataset.go === runtime.scene);
    });
  }

  function placeMascotIntoSlot() {
    const cs = colEl.querySelector('.cs') || $('.cs');
    if (!cs || !mkCanvas || !isl) {
      if (mkCanvas) mkCanvas.style.opacity = '0';
      return;
    }
    const rIsl = isl.getBoundingClientRect();
    const rC = cs.getBoundingClientRect();
    if (!rC.width || !rC.height) {
      mkCanvas.style.opacity = '0';
      return;
    }
    const sc = Math.min(rC.width, rC.height) / 200;
    const x = rC.left - rIsl.left - (isl.clientLeft || 0) + (rC.width - 200 * sc) / 2;
    const y = rC.top - rIsl.top - (isl.clientTop || 0) + (rC.height - 200 * sc) / 2;
    slotPos = { x, y, s: sc, ready: true };
    mkCanvas.style.opacity = '1';
    mkCanvas.style.transform = `translate(${x.toFixed(1)}px,${y.toFixed(1)}px) scale(${sc.toFixed(4)})`;
  }

  let lastWinW = 0;
  let lastWinH = 0;
  let fittingNow = false;
  let hoverPinned = false;
  let hoverInside = false;
  let lastActivityAt = Date.now();
  let shrinkTimer = 0;
  let pendingShrinkTarget = '';
  let hoverEnterTimer = 0;
  let hoverLeaveTimer = 0;

  function markActivity() {
    lastActivityAt = Date.now();
  }

  function closeOpenOverlays(refit = false) {
    let changed = false;
    const ddOpen = $('#ag-chat-dd.open');
    if (ddOpen) {
      ddOpen.classList.remove('open');
      $('#ag-chat-pill')?.setAttribute('aria-expanded', 'false');
      changed = true;
    }
    const crtOpen = $('#crt-panel.open');
    if (crtOpen) {
      crtOpen.classList.remove('open');
      $('#crt-pill')?.classList.remove('open');
      $('#crt-pill')?.setAttribute('aria-expanded', 'false');
      changed = true;
    }
    if (changed && refit) requestAnimationFrame(fitIsland);
    return changed;
  }

  function collapseIsland() {
    global.__suppress_hover_until = Date.now() + 1100;
    closeOpenOverlays(false);
    runtime.editingDraft = false;
    try { document.activeElement?.blur?.(); } catch (_) {}
    hoverInside = false;
    hoverPinned = false;
    clearTimeout(hoverEnterTimer);
    clearTimeout(hoverLeaveTimer);
    global.Bridge?.windowControl?.('blur_input');
    go('compact');
  }
  let lastCursorOutside = null;
  let cachedHitRect = { left: 17, top: 0, right: 285, bottom: 44 };

  function syncCursorRegion(outside) {
    const compact = runtime.scene === 'compact';
    lastCursorOutside = !!outside && compact;
    global.Bridge?.cursorRegion?.(lastCursorOutside, cachedHitRect, compact);
  }

  function fitIsland() {
    if (!isl || !innerEl || fittingNow) return;
    fittingNow = true;
    try {
      const prevW = isl.style.width;
      const prevH = isl.style.height;
      isl.style.width = 'auto';
      isl.style.height = 'auto';

      const r = innerEl.getBoundingClientRect();
      const isCompact = runtime.scene === 'compact';
      const rawW = Math.max(innerEl.offsetWidth || 0, Math.ceil(r.width || 0));
      const rawH = Math.max(innerEl.offsetHeight || 0, Math.ceil(r.height || 0));
      const w = isCompact
        ? Math.max(268, Math.min(460, Math.ceil(rawW)))
        : Math.max(340, Math.min(760, Math.ceil(rawW)));
      const h = isCompact
        ? 44
        : Math.max(88, Math.min(680, Math.ceil(rawH + 4)));

      isl.style.width = prevW || '268px';
      isl.style.height = prevH || '44px';
      void isl.offsetWidth;

      isl.style.width = w + 'px';
      isl.style.height = h + 'px';
      isl.style.borderRadius = isCompact ? '0 0 var(--island-radius-compact) var(--island-radius-compact)' : '0 0 var(--island-radius-open) var(--island-radius-open)';

      const ir = isl.getBoundingClientRect();
      let extraRight = 17 + w;
      let extraBottom = h;
      ['#ag-chat-dd.open', '#crt-panel.open'].forEach(sel => {
        const ov = $(sel);
        if (!ov) return;
        const ovR = ov.getBoundingClientRect();
        if (ovR.width > 0 && ovR.height > 0) {
          extraBottom = Math.max(extraBottom, Math.ceil(ovR.bottom - ir.top + 14));
          extraRight = Math.max(extraRight, Math.ceil(17 + (ovR.right - ir.left) + 8));
        }
      });

      const winW = Math.max(w + 34, extraRight + 17);
      const winH = Math.min(760, Math.max(h, extraBottom) + 18);
      cachedHitRect = { left: 17, top: 0, right: extraRight, bottom: Math.max(h, extraBottom) };
      const shrinking = isCompact && ((lastWinW > 0 && winW < lastWinW) || (lastWinH > 0 && winH < lastWinH));
      if (winW !== lastWinW || winH !== lastWinH) {
        if (shrinking) {
          const targetKey = `${winW}x${winH}`;
          if (pendingShrinkTarget !== targetKey) {
            clearTimeout(shrinkTimer);
            pendingShrinkTarget = targetKey;
            shrinkTimer = setTimeout(() => {
              pendingShrinkTarget = '';
              if (runtime.scene === 'compact') {
                lastWinW = winW;
                lastWinH = winH;
                global.Bridge?.resizeWindow?.(winW, winH);
                syncCursorRegion(!hoverInside && runtime.scene === 'compact');
              }
            }, 290);
          }
        } else {
          clearTimeout(shrinkTimer);
          pendingShrinkTarget = '';
          lastWinW = winW;
          lastWinH = winH;
          global.Bridge?.resizeWindow?.(winW, winH);
        }
      } else {
        clearTimeout(shrinkTimer);
        pendingShrinkTarget = '';
      }
      if (!shrinking) {
        syncCursorRegion(!hoverInside && isCompact);
      }
      requestAnimationFrame(placeMascotIntoSlot);
      setTimeout(placeMascotIntoSlot, 200);
      setTimeout(placeMascotIntoSlot, 640);
    } finally {
      fittingNow = false;
    }
  }

  function isEditingInsideIsland() {
    const ae = document.activeElement;
    if (ae && isl?.contains(ae) && ['INPUT', 'TEXTAREA', 'SELECT'].includes(ae.tagName)) {
      return true;
    }
    if (runtime.editingDraft) return true;
    if ($('#ag-chat-dd.open') || $('#crt-panel.open')) return true;
    return false;
  }

  function canAutoCollapseNow() {
    if (['compact', 'boot', 'approve', 'question'].includes(runtime.scene)) return false;
    if (curAg && getAgChat(curAg().id)?.busy) return false;
    const delaySec = Number(CFG.autoclose ?? 15);
    const idleLongEnough = delaySec > 0 && (Date.now() - lastActivityAt) >= delaySec * 1000;
    if (idleLongEnough) return true;
    if (hoverInside || isEditingInsideIsland()) return false;
    return true;
  }

  function resetHoldTimer() {
    clearTimeout(holdTimer);
    const delaySec = Number(CFG.autoclose ?? 15);
    if (delaySec <= 0) return;
    if (['compact', 'boot', 'approve', 'question'].includes(runtime.scene)) return;
    const elapsed = Date.now() - lastActivityAt;
    const waitMs = Math.max(800, delaySec * 1000 - elapsed);
    holdTimer = setTimeout(() => {
      if (canAutoCollapseNow()) {
        collapseIsland();
      } else {
        resetHoldTimer();
      }
    }, waitMs);
  }

  function go(name, opt = {}) {
    markActivity();
    const target = global.MikuScenes.SC[name] ? name : 'compact';
    runtime.previousScene = runtime.scene;
    runtime.scene = target;

    if (['prompt', 'settings', 'approve', 'agent_delegate'].includes(target)) {
      global.Bridge?.windowControl?.('focus_input');
    } else if (target === 'compact' || target === 'overview') {
      global.Bridge?.windowControl?.('blur_input');
    }

    if (target === 'prompt') {
      const c = getAgChat(curAg().id);
      c.unread = false;
      saveAgChats();
    }

    if (target === 'compact') {
      isl.classList.add('closing');
      clearTimeout(closingTimer);
      closingTimer = setTimeout(() => isl.classList.remove('closing'), 480);
      innerEl.className = 'm';
    } else if (['boot', 'intro'].includes(target)) {
      innerEl.className = 'stg';
    } else {
      innerEl.className = 'hc';
    }

    colEl.innerHTML = global.MikuScenes.SC[target]();
    global.MikuScenes.postMount(target, colEl, { go, rerender: () => go(runtime.scene, { keepState: true }), fit: fitIsland });

    if (!opt.keepState) {
      if (target === 'approve') global.MikuMascot?.setState?.('approve');
      else if (target === 'alert') global.MikuMascot?.setState?.('error');
      else if (target === 'compact') global.MikuMascot?.setState?.(ST.mic ? 'listening' : 'idle');
    }

    fitIsland();
    syncDockActive();
    resetHoldTimer();
  }

  function updateIslandGlow(stateName) {
    const active = ['listening', 'thinking', 'talking', 'approve', 'error'].includes(stateName) || !!ST.mic || !!runtime.thk.on;
    isl.classList.toggle('vz', active);
    if (stateName === 'listening' || ST.mic) isl.style.setProperty('--vc', '#f5f5f7');
    else if (stateName === 'thinking' || runtime.thk.on) isl.style.setProperty('--vc', '#a88cff');
    else if (stateName === 'approve' || stateName === 'error') isl.style.setProperty('--vc', '#ffb300');
    else isl.style.setProperty('--vc', '#a88cff');
  }

  let turnWatchdogTimer = null;

  function finalizeTurn(optsOrReason = {}) {
    const opts = typeof optsOrReason === 'string' ? { reason: optsOrReason } : (optsOrReason || {});
    const reason = opts.reason || 'ok';

    if (turnWatchdogTimer) {
      clearTimeout(turnWatchdogTimer);
      turnWatchdogTimer = null;
    }
    if (runtime.turnWatchdogTimer) {
      clearTimeout(runtime.turnWatchdogTimer);
      runtime.turnWatchdogTimer = null;
    }
    runtime.thk.on = 0;
    runtime.thk.end = Date.now();
    if (runtime.thk.start) {
      runtime.thk.dur = Math.max(0.1, (runtime.thk.end - runtime.thk.start) / 1000);
    }

    const ag = curAg();
    const ch = getAgChat(ag.id);
    const wasBusy = Boolean(ch.busy || opts.wasBusy);
    ch.busy = false;

    const restState = ST.mic ? 'listening' : 'idle';
    global.MikuMascot?.setState?.(restState);

    if (reason === 'watchdog_timeout') {
      if (!Array.isArray(ch.history)) ch.history = [];
      ch.history.push({
        role: 'agent',
        text: 'Tiempo de espera agotado (30s watchdog). El agente no respondió.',
        error: true,
        time: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      });
      global.SoundFX?.error?.();
    } else if (reason === 'disconnect' && wasBusy) {
      if (!Array.isArray(ch.history)) ch.history = [];
      ch.history.push({
        role: 'agent',
        text: 'Conexión perdida con el daemon.',
        error: true,
        time: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      });
      global.SoundFX?.error?.();
    }

    if (reason !== 'cancel' && (reason !== 'disconnect' || wasBusy)) {
      const isOk = reason === 'ok' || (reason === 'reply' && !opts.error);
      global.MikuMascot?.emote?.(isOk ? 'happy' : 'error', 1400);
    }

    saveAgChats();
    updateIslandGlow(global.MikuMascot?.getState?.() || restState);

    if (opts.noRerender) return;

    if (runtime.scene === 'prompt') {
      if (document.activeElement?.id === 'qi' || runtime.editingDraft) {
        runtime.refocusChatInput = true;
      }
      go('prompt', { keepState: true });
    } else if ((reason === 'ok' || reason === 'reply') && runtime.scene !== 'compact') {
      runtime.resText = ch.lastA || '';
      go('result', { keepState: true });
    } else {
      go(runtime.scene, { keepState: true });
    }
  }

  global.MikuUI.finalizeTurn = finalizeTurn;
  global.MikuUI.onVoiceTranscript = ({ text, isFinal }) => {
    handleFeedMessage({ type: 'voice_transcript', text, is_final: Boolean(isFinal) });
  };
  global.MikuUI.onVoiceConsentOpen = (open) => {
    runtime.voiceConsentOpen = Boolean(open);
    if (open && runtime.scene !== 'prompt') {
      go('prompt');
    } else {
      go(runtime.scene, { keepState: true });
    }
  };
  global.MikuUI.onVoiceStatus = (info) => {
    if (info?.status === 'fallback') {
      global.SoundFX?.pop?.();
      go(runtime.scene, { keepState: true });
    }
  };

  function handleFeedMessage(msg) {
    if (!msg || typeof msg !== 'object') return;
    if (msg.type === 'ws') {
      const prevWs = ST.ws;
      const nextWs = msg.ok ? 1 : 0;
      ST.ws = nextWs;
      if (!msg.ok) {
        const ag = curAg();
        const ch = getAgChat(ag.id);
        finalizeTurn({ reason: 'disconnect', wasBusy: Boolean(ch?.busy) });
      }
      if (runtime.scene === 'boot' && msg.ok) {
        LOCKI = 0;
        go(isWebViewHost ? 'compact' : 'overview');
        return;
      }
      if (prevWs === nextWs) {
        return;
      }
      const activeTag = document.activeElement?.tagName;
      if ((runtime.scene === 'settings' || runtime.scene === 'prompt') &&
          (activeTag === 'INPUT' || activeTag === 'TEXTAREA' || activeTag === 'SELECT')) {
        const badge = document.getElementById('h-status') || document.querySelector('.wsdot');
        if (badge) {
          badge.classList.toggle('ok', Boolean(msg.ok));
          if (badge.id === 'h-status') badge.textContent = msg.ok ? '47400 OK' : 'Offline';
        }
        return;
      }
      if (['overview', 'prompt', 'settings', 'link', 'compact', 'boot'].includes(runtime.scene)) {
        go(runtime.scene, { keepState: true });
      }
      return;
    }

    if (msg.type === 'agent_state') {
      const state = msg.state || 'idle';
      global.MikuMascot?.setState?.(state);
      runtime.thk.on = (state === 'thinking') ? 1 : 0;
      updateIslandGlow(state);
      return;
    }

    if (msg.type === 'voice_transcript') {
      const txt = msg.text || '';
      const inp = document.getElementById('qi');
      const ag = curAg();
      const ch = getAgChat(ag.id);
      if (txt) {
        if (inp) {
          inp.value = txt;
          if (typeof Event === 'function') {
            try { inp.dispatchEvent(new Event('input', { bubbles: true })); } catch (_) {}
          }
        }
        if (ch) ch.draft = txt;
      }
      if (msg.is_final && CFG.voice_auto_send && txt) {
        const submitFn = global.MikuUI?.submitChatPrompt || global.MikuChat?.submitChatPrompt;
        if (typeof submitFn === 'function') {
          if (runtime.scene !== 'prompt') {
            go('prompt', { keepState: true });
          }
          submitFn({ rerender: () => go('prompt', { keepState: true }) }, txt);
        }
      }
      return;
    }

    if (msg.type === 'cancelled') {
      finalizeTurn({ reason: 'cancel' });
      return;
    }

    if (msg.type === 'state_sync') {
      const hasInline = Array.isArray(msg.secrets) || Array.isArray(msg.agents);
      const syncPromise = hasInline
        ? Promise.resolve(global.Bridge?.applySnapshot?.(msg))
        : global.Bridge?.syncAll?.().catch(() => {});
      syncPromise.then(() => {
        syncMascotToAgent(curAg());
        global.MikuUI.applyThemeVars?.(CFG);
        const activeTag = document.activeElement?.tagName;
        const secVal = document.getElementById('sec-val')?.value;
        const chatInp = document.getElementById('qi')?.value;
        const editingDraft =
          runtime.editingDraft ||
          ((runtime.scene === 'settings' || runtime.scene === 'prompt') &&
            (activeTag === 'INPUT' || activeTag === 'TEXTAREA' || activeTag === 'SELECT' || Boolean(secVal) || Boolean(chatInp)));
        if (!editingDraft) go(runtime.scene, { keepState: true });
      });
      return;
    }

    if (msg.type === 'agent_reply') {
      const agId = msg.agent_id || curAg().id;
      const ch = getAgChat(agId);
      if (msg.reply_id && Array.isArray(ch.history) && ch.history.some(h => h.replyId === msg.reply_id)) {
        finalizeTurn({ reason: msg.error ? 'error' : 'ok', error: Boolean(msg.error) });
        return;
      }
      ch.lastA = msg.text || '';
      ch.thinkDur = ch.thinkStart ? Math.max(0.2, (Date.now() - ch.thinkStart) / 1000) : 0;
      if (Array.isArray(msg.steps)) ch.steps = msg.steps;
      if (!Array.isArray(ch.history)) ch.history = [];
      ch.history.push({
        replyId: msg.reply_id || null,
        role: 'agent',
        text: ch.lastA,
        error: Boolean(msg.error),
        retryPrompt: msg.retry_prompt || (msg.error ? (ch.lastQ || runtime.lastQ || '') : ''),
        localRoute: Boolean(msg.routed_local ?? msg.local_route),
        savedTokens: Number(msg.tokens_saved || 0),
        steps: msg.steps || [],
        thinkDur: ch.thinkDur,
        time: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      });
      if (runtime.scene !== 'prompt' || curAg().id !== agId) ch.unread = true;
      global.SoundFX?.[msg.error ? 'error' : 'receive']?.();
      if (!msg.error && ch.lastA && (CFG.voice_tts_enabled ?? CFG.voice ?? true)) {
        global.SoundFX?.speakReply?.(ch.lastA);
      }
      finalizeTurn({ reason: 'reply', error: Boolean(msg.error) });
      return;
    }

    if (msg.type === 'approve') {
      runtime.pendingApproveId = msg.id || 'req-1';
      runtime.pendingCmd = msg.cmd || msg.command || '';
      runtime.pendingDiff = msg.diff || '';
      global.SoundFX?.hold?.();
      go('approve');
      return;
    }

    if (msg.type === 'question') {
      runtime.question = {
        title: msg.title || 'Pregunta de Miku',
        text: msg.text || 'Elige una opción:',
        options: Array.isArray(msg.options) ? msg.options : ['Continuar', 'Cancelar']
      };
      global.SoundFX?.pop?.();
      go('question');
      return;
    }

    if (msg.type === 'error_card') {
      finalizeTurn({ reason: 'error', noRerender: true });
      runtime.errorCard = {
        title: msg.title || 'Error de ejecución',
        exitCode: msg.exit_code ?? msg.exitCode ?? 1,
        stderr: msg.stderr || msg.text || '',
        retryPrompt: msg.retry_prompt || runtime.lastQ || ''
      };
      global.SoundFX?.error?.();
      go('error_card');
      return;
    }

    if (msg.type === 'agent_work') {
      runtime.agentWork = {
        taskId: msg.task_id || '',
        agentId: msg.agent_id || 'miku',
        agentName: msg.agent_name || msg.agent_id || 'Miku',
        mode: msg.mode || 'code',
        title: msg.title || 'Tarea de agente',
        status: msg.status || 'running',
        result: msg.result || '',
        phases: Array.isArray(msg.phases) ? msg.phases : []
      };
      global.SoundFX?.[msg.status === 'done' ? 'chime' : 'pop']?.();
      if (runtime.scene === 'agent_work' || msg.open !== false) go('agent_work');
      return;
    }

    if (msg.type === 'ai_inference') {
      runtime.aiInf = {
        provider: msg.provider || CFG.provider,
        model: msg.model || CFG.model,
        status: msg.status || 'running',
        latencyMs: msg.latency_ms ?? 0,
        inputTokens: msg.input_tokens ?? null,
        outputTokens: msg.output_tokens ?? null,
        host: msg.host || ''
      };
      if (runtime.scene === 'ai_inference') go('ai_inference', { keepState: true });
      return;
    }

    if (msg.type === 'mcp_download') {
      runtime.mcpDl = {
        id: msg.id || 'mcp',
        name: msg.name || 'Servidor MCP',
        progress: msg.progress ?? 0,
        status: msg.status || 'configuring'
      };
      global.SoundFX?.[runtime.mcpDl.progress >= 100 ? 'chime' : 'pop']?.();
      go('mcp_download');
      return;
    }

    if (msg.type === 'update_card') {
      runtime.updateCard = {
        version: msg.version || 'al día',
        channel: msg.channel || 'stable',
        notes: msg.notes || ''
      };
      go('update_card');
      return;
    }

    if (msg.type === 'web_grounding') {
      runtime.webGround = {
        query: msg.query || '',
        summary: msg.summary || '',
        sources: Array.isArray(msg.sources) ? msg.sources : []
      };
      go('web_grounding');
      return;
    }

    if (msg.type === 'tool_use') {
      runtime.toolName = msg.tool || 'Tool';
      runtime.toolCmd = msg.cmd || '';
      go('tool_use');
      return;
    }

    if (msg.type === 'notify') {
      runtime.noteTitle = msg.title || 'Miku';
      runtime.noteBody = msg.body || '';
      global.SoundFX?.chime?.();
      go('notify');
      return;
    }

    if (msg.type === 'alert') {
      runtime.alertTitle = msg.title || 'Gatekeeper';
      runtime.alertBody = msg.body || '';
      global.SoundFX?.error?.();
      go('alert');
      return;
    }

    if (msg.type === 'live_status') {
      runtime.computerLiveStatus = msg.text || '';
      if (runtime.scene === 'compact') go('compact', { keepState: true });
      return;
    }

    if (msg.type === 'music') {
      Object.assign(MUS, msg.data || msg);
      runtime.ovPlayer?.sync?.();
      runtime.plPlayer?.sync?.();
      if (typeof MUS.vol === 'number') {
        runtime.ovSlosh?.set?.(MUS.vol);
      }
      if (runtime.scene === 'compact') go('compact', { keepState: true });
      return;
    }

    if (msg.type === 'mascot') {
      if (msg.state) global.MikuMascot?.setState?.(msg.state);
      if (msg.emote) global.MikuMascot?.emote?.(msg.emote, msg.ms || 1400);
      return;
    }

    if (msg.type === 'outside_click') {
      closeOpenOverlays(true);
      if (!['compact', 'boot', 'approve', 'question'].includes(runtime.scene)) {
        collapseIsland();
      }
      return;
    }

    if (msg.type === 'user_idle') {
      if (!['compact', 'boot', 'approve', 'question'].includes(runtime.scene) && !getAgChat(curAg().id)?.busy) {
        collapseIsland();
      }
      return;
    }

    if (msg.type === 'cursor') {
      if (slotPos.ready && isl && typeof msg.lx === 'number' && typeof msg.ly === 'number') {
        const r = isl.getBoundingClientRect();
        const cx = r.left + slotPos.x + 100 * slotPos.s;
        const cy = r.top + slotPos.y + 100 * slotPos.s;
        global.MikuMascot?.lookAt?.((msg.lx - cx) / 260, (msg.ly - cy) / 220);
      } else if (typeof msg.dx === 'number' && typeof msg.dy === 'number') {
        global.MikuMascot?.lookAt?.(msg.dx, msg.dy);
      }
      if (typeof msg.inside === 'boolean') {
        if (msg.inside) {
          hoverInside = true;
          clearTimeout(hoverLeaveTimer);
          clearTimeout(holdTimer);
          global.MikuMascot?.wakeFromRest?.();
          if (CFG.hover_open && runtime.scene === 'compact' && Date.now() >= (global.__suppress_hover_until || 0)) {
            clearTimeout(hoverEnterTimer);
            hoverEnterTimer = setTimeout(() => {
              if (hoverInside && CFG.hover_open && runtime.scene === 'compact' && Date.now() >= (global.__suppress_hover_until || 0)) {
                hoverPinned = false;
                go('overview');
              }
            }, 140);
          }
        } else if (hoverInside) {
          hoverInside = false;
          clearTimeout(hoverEnterTimer);
          clearTimeout(hoverLeaveTimer);
          if (!isEditingInsideIsland()) hoverPinned = false;
          resetHoldTimer();
          if (CFG.hover_open && !hoverPinned && runtime.scene === 'overview' && !curAg()?.busy) {
            hoverLeaveTimer = setTimeout(() => {
              if (!hoverInside && !hoverPinned && CFG.hover_open && runtime.scene === 'overview') {
                collapseIsland();
              }
            }, 700);
          }
        }
      }
      return;
    }

    if (msg.type === 'voice_consent_required') {
      global.MikuMascot?.wakeFromRest?.();
      global.MikuVoiceRecognizer?.openConsent?.();
      return;
    }

    if (msg.type === 'wake_ptt') {
      global.MikuMascot?.wakeFromRest?.();
      if (global.MikuVoiceRecognizer) {
        const res = global.MikuVoiceRecognizer.toggle();
        if (res?.started || res?.consentRequired) go('prompt', { keepState: true });
      } else {
        runtime.voiceConsentOpen = true;
        go('prompt');
      }
      return;
    }

    if (msg.type === 'open_onboarding' || msg.type === 'open_settings') {
      global.MikuMascot?.wakeFromRest?.();
      global.MikuMascot?.emote?.('happy', 1200);
      go('settings');
      return;
    }

    if (msg.type === 'open_overview') {
      global.MikuMascot?.wakeFromRest?.();
      go('overview');
      return;
    }

    if (msg.type === 'island_control') {
      const action = String(msg.action || '').toLowerCase();
      if (action === 'expand' || action === 'show') {
        global.MikuMascot?.wakeFromRest?.();
        go(msg.scene || 'overview');
      } else if (action === 'collapse' || action === 'minimize' || action === 'hide') {
        collapseIsland();
      } else if (action === 'ping') {
        global.MikuMascot?.wakeFromRest?.();
        global.MikuMascot?.emote?.('happy', 1800);
        global.SoundFX?.chime?.();
        runtime.ql = { title: msg.title || 'Miku-Link', text: msg.text || 'Señal recibida desde tu celular.' };
        go('quicklook');
      }
    }
  }

  function handleSelectedFile(file) {
    if (!file) return;
    runtime.fn = file.name;
    runtime.uploadPct = 15;
    go('uploading');
    const reader = new FileReader();
    reader.onprogress = ev => {
      if (ev.lengthComputable) {
        runtime.uploadPct = Math.round((ev.loaded / ev.total) * 70);
        if (runtime.scene === 'uploading') go('uploading', { keepState: true });
      }
    };
    reader.onload = async () => {
      const dataUrl = String(reader.result || '');
      runtime.uploadPct = 90;
      const ag = curAg();
      const ch = getAgChat(ag.id);
      ch.lastFile = { name: file.name, size: file.size, type: file.type, content: dataUrl.slice(0, 64000) };
      saveAgChats();
      await global.Bridge.uploadFile(file.name, dataUrl).catch(() => null);
      runtime.uploadPct = 100;
      global.MikuMascot?.emote?.('happy', 1100);
      global.SoundFX?.chime?.();
      go('prompt');
    };
    reader.readAsDataURL(file);
  }

  async function init() {
    await loadLocale(CFG.lang || 'es');
    let mascotCfg = null;
    try {
      const r = await fetch('/shared/mascot/mascot.config.json');
      if (r.ok) mascotCfg = await r.json();
    } catch (_) {}
    runtime.mascotConfig = mascotCfg;

    global.MikuMascot.mount(mkCanvas, {
      config: mascotCfg,
      onStateChange: s => updateIslandGlow(s),
      onRestChange: resting => {
        isl.classList.toggle('resting', !!resting);
        global.Bridge?.reportMascotAwake?.(!resting);
      },
      getBadges: () => ({ unread: !!getUnreadAgent(), approve: runtime.scene === 'approve' }),
      canSleep: () => runtime.scene === 'compact' && !ST.mic && !runtime.thk.on,
      isStateLocked: () => !!LOCKI,
      canRest: (_now, info) => runtime.scene === 'compact' && !MUS.playing && !ST.mic && info.base === 'idle' && !info.eatOn,
      onAfterFrame: (_dt, now) => {
        if (MUS.playing && runtime.scene === 'compact') {
          const star = colEl.querySelector('canvas.star');
          if (star) global.MikuComponents.drawStar(star, now);
        }
      }
    });
    syncMascotToAgent(curAg());

    global.addEventListener('pointermove', e => {
      const insideIsl = !!e.target?.closest?.('#isl');
      if (runtime.scene === 'compact' && lastCursorOutside !== !insideIsl) {
        syncCursorRegion(!insideIsl);
      } else if (runtime.scene !== 'compact' && lastCursorOutside) {
        syncCursorRegion(false);
      }
      if (!slotPos.ready || !isl) return;
      const r = isl.getBoundingClientRect();
      const cx = r.left + slotPos.x + 100 * slotPos.s;
      const cy = r.top + slotPos.y + 100 * slotPos.s;
      global.MikuMascot?.lookAt?.((e.clientX - cx) / 260, (e.clientY - cy) / 220);
    }, { passive: true });

    document.addEventListener('pointerleave', () => {
      hoverInside = false;
      if (runtime.scene === 'compact' && !lastCursorOutside) {
        syncCursorRegion(true);
      }
    });

    isl.addEventListener('pointerenter', () => {
      if (Date.now() < (global.__suppress_hover_until || 0)) return;
      hoverInside = true;
      clearTimeout(hoverLeaveTimer);
      clearTimeout(holdTimer);
      global.MikuMascot?.wakeFromRest?.();
      if (runtime.scene === 'compact' && CFG.hover_open) {
        clearTimeout(hoverEnterTimer);
        hoverEnterTimer = setTimeout(() => {
          if (hoverInside && runtime.scene === 'compact' && CFG.hover_open && Date.now() >= (global.__suppress_hover_until || 0)) {
            hoverPinned = false;
            go('overview');
          }
        }, 140);
      }
    });

    isl.addEventListener('pointerdown', e => {
      markActivity();
      clearTimeout(holdTimer);
      const interactiveForm = e.target?.closest?.('input, textarea, select, .crt-stage, .ag-chat-pill-wrap');
      if (interactiveForm && runtime.scene !== 'compact') {
        hoverPinned = true;
      } else {
        hoverPinned = false;
      }
    });

    // Clic fuera del Island -> colapsa instantáneamente (navegador y WebView)
    global.addEventListener('pointerdown', e => {
      if (e.target?.closest?.('#isl, .preview-dock, .zoom-modal-backdrop')) return;
      closeOpenOverlays(false);
      if (!['compact', 'boot', 'approve', 'question'].includes(runtime.scene)) {
        collapseIsland();
      }
    }, { capture: true });

    global.addEventListener('blur', () => {
      setTimeout(() => {
        if (document.hasFocus()) return;
        closeOpenOverlays(false);
        if (!['compact', 'boot', 'approve', 'question'].includes(runtime.scene) && !getAgChat(curAg().id)?.busy) {
          collapseIsland();
        }
      }, 110);
    });

    isl.addEventListener('pointerleave', () => {
      hoverInside = false;
      clearTimeout(hoverEnterTimer);
      clearTimeout(hoverLeaveTimer);
      if (!isEditingInsideIsland()) hoverPinned = false;
      resetHoldTimer();
      if (CFG.hover_open && !hoverPinned && runtime.scene === 'overview' && !curAg()?.busy) {
        hoverLeaveTimer = setTimeout(() => {
          if (!hoverInside && !hoverPinned && CFG.hover_open && runtime.scene === 'overview') {
            collapseIsland();
          }
        }, 700);
      }
    });

    ['keydown', 'input', 'focusin'].forEach(evName => {
      isl.addEventListener(evName, () => {
        markActivity();
        resetHoldTimer();
      }, { passive: true });
    });

    isl.addEventListener('focusout', () => {
      setTimeout(() => {
        if (!isEditingInsideIsland() && !hoverInside) {
          hoverPinned = false;
          resetHoldTimer();
        }
      }, 120);
    });

    // Clic global delegado (sin inline onclick)
    document.addEventListener('click', async e => {
      const agPill = e.target.closest('#ag-chat-pill');
      if (agPill) {
        e.stopPropagation();
        const ddMenu = $('#ag-chat-dd');
        if (ddMenu) {
          const open = ddMenu.classList.toggle('open');
          agPill.setAttribute('aria-expanded', String(open));
          global.SoundFX?.click?.();
          requestAnimationFrame(fitIsland);
        }
        return;
      }

      const selAgBtn = e.target.closest('[data-select-agent]');
      if (selAgBtn) {
        setSela?.(Number(selAgBtn.dataset.selectAgent));
        const ag = curAg();
        getAgChat(ag.id).unread = false;
        saveAgChats();
        syncMascotToAgent(ag);
        global.MikuMascot?.impulse?.(0.2, -70);
        global.MikuMascot?.emote?.('happy', 800);
        global.SoundFX?.tab?.();
        go(runtime.scene, { keepState: true });
        return;
      }

      const ddOpen = $('#ag-chat-dd.open');
      if (ddOpen && !e.target.closest('.ag-chat-pill-wrap')) {
        ddOpen.classList.remove('open');
        $('#ag-chat-pill')?.setAttribute('aria-expanded', 'false');
        requestAnimationFrame(fitIsland);
      }

      const openSetWin = e.target.closest('[data-open-settings]');
      if (openSetWin) {
        global.SoundFX?.click?.();
        const cat = openSetWin.dataset.openSettings || null;
        global.Bridge?.openSettings?.(cat);
        return;
      }

      const goSet = e.target.closest('[data-go-settings]');
      if (goSet) {
        global.SoundFX?.click?.();
        LOCKI = 0;
        global.Bridge?.openSettings?.(goSet.dataset.goSettings || 'general');
        return;
      }

      const goEl = e.target.closest('[data-go]');
      if (goEl) {
        global.SoundFX?.click?.();
        LOCKI = 0;
        go(goEl.dataset.go);
        return;
      }

      if (e.target.closest('.compact-trigger') || (runtime.scene === 'compact' && e.target.closest('.island') && !e.target.closest('button'))) {
        global.SoundFX?.pop?.();
        go('overview');
        return;
      }

      const outBtn = e.target.closest('[data-outfit]');
      if (outBtn) {
        const outId = outBtn.dataset.outfit || 'none';
        const ag = curAg();
        const isPrin = !ag || ag === PRIN || ag.id === 'miku';
        ag.outfit = outId;
        if (isPrin) {
          PRIN.outfit = outId;
          global.MikuMascot?.setOutfit?.(outId, true);
          global.Bridge?.saveAgentsBackend?.(ST.agents);
        } else {
          global.MikuMascot?.setOutfit?.(outId, false);
          saveAgents?.();
        }
        global.Bridge?.broadcast?.({ k: 'mascot', agentId: ag.id, outfit: outId });
        document.querySelectorAll('#col .wb').forEach(b => b.classList.toggle('on', b.dataset.outfit === outId));
        const wnEl = document.getElementById('wn');
        if (wnEl) wnEl.textContent = OL[outId] || outId;
        global.MikuMascot?.emote?.('happy', 1100);
        global.SoundFX?.pop?.();
        resetHoldTimer();
        return;
      }

      const bodyBtn = e.target.closest('[data-body-color]');
      if (bodyBtn) {
        const ag = curAg();
        if (!ag || ag === PRIN || ag.id === 'miku') return;
        const hex = bodyBtn.dataset.bodyColor;
        if (usedByColor(hex, ag)) {
          global.MikuMascot?.impulse?.(0.12, -30);
          return;
        }
        ag.c = hex;
        saveAgents?.();
        syncMascotToAgent(ag);
        global.Bridge?.broadcast?.({ k: 'mascot', agentId: ag.id, color: hex });
        global.MikuMascot?.impulse?.(0.2, -70);
        global.MikuMascot?.emote?.('happy', 900);
        global.SoundFX?.pop?.();
        go('wardrobe', { keepState: true });
        return;
      }

      const galBtn = e.target.closest('[data-gal-state]');
      if (galBtn) {
        global.MikuMascot?.setState?.(galBtn.dataset.galState);
        global.SoundFX?.pop?.();
        go('gallery', { keepState: true });
        return;
      }

      const pgDot = e.target.closest('[data-pg]');
      if (pgDot) {
        runtime.pg = ((Number(pgDot.dataset.pg) || 0) % 2 + 2) % 2;
        global.SoundFX?.tab?.();
        go('overview', { keepState: true });
        return;
      }

      const pgStep = e.target.closest('[data-pg-step]');
      if (pgStep) {
        runtime.pg = (runtime.pg + Number(pgStep.dataset.pgStep) + 2) % 2;
        global.SoundFX?.tab?.();
        go('overview', { keepState: true });
        return;
      }

      const ovAg = e.target.closest('[data-ov-agent]');
      if (ovAg) {
        global.MikuUI.setSela(Number(ovAg.dataset.ovAgent));
        syncMascotToAgent(curAg());
        go('prompt');
        return;
      }

      const apBtn = e.target.closest('[data-approve-id]');
      if (apBtn) {
        global.Bridge?.choice?.(apBtn.dataset.approveId, apBtn.dataset.verdict);
        global.SoundFX?.chime?.();
        go('overview');
        return;
      }

      const chBtn = e.target.closest('[data-choose-val]');
      if (chBtn) {
        global.Bridge?.choice?.('choose', chBtn.dataset.chooseVal);
        go('overview');
        return;
      }

      const qBtn = e.target.closest('[data-q-idx]');
      if (qBtn) {
        const idx = Number(qBtn.dataset.qIdx);
        const val = qBtn.dataset.qVal || '';
        global.SoundFX?.chime?.();
        await global.Bridge?.sendUiChoice?.('question', val, idx, runtime.question?.title || '').catch(() => null);
        go('overview');
        return;
      }

      const dAg = e.target.closest('[data-delegate-agent]');
      if (dAg) {
        runtime.delegateAgentId = dAg.dataset.delegateAgent;
        global.SoundFX?.click?.();
        go('agent_delegate', { keepState: true });
        return;
      }

      const dMode = e.target.closest('[data-delegate-mode]');
      if (dMode) {
        runtime.delegateMode = dMode.dataset.delegateMode;
        global.SoundFX?.click?.();
        go('agent_delegate', { keepState: true });
        return;
      }

      if (e.target.id === 'delegate-send-btn') {
        const ta = $('#delegate-task-input');
        const task = ta?.value?.trim();
        if (!task) return;
        const targetId = runtime.delegateAgentId || 'miku';
        const mode = runtime.delegateMode || 'code';
        runtime.agentWork = {
          taskId: 'pending',
          agentId: targetId,
          agentName: targetId,
          mode,
          title: task,
          status: 'running',
          result: '',
          phases: [
            { label: 'Contexto SQLite', state: 'done' },
            { label: `Agente ${targetId}`, state: 'active' },
            { label: 'Validación Gatekeeper', state: 'pending' }
          ]
        };
        global.SoundFX?.pop?.();
        go('agent_work');
        const res = await global.Bridge.delegateTask({ fromAgent: curAg().id, toAgent: targetId, task, mode }).catch(err => ({ ok: false, error: String(err) }));
        if (res?.work) {
          runtime.agentWork = {
            taskId: res.work.task_id || '',
            agentId: res.work.agent_id || targetId,
            agentName: res.work.agent_name || targetId,
            mode: res.work.mode || mode,
            title: res.work.title || task,
            status: res.work.status || 'done',
            result: res.work.result || '',
            phases: Array.isArray(res.work.phases) ? res.work.phases : []
          };
          global.SoundFX?.chime?.();
          go('agent_work', { keepState: true });
        } else if (res && res.ok === false) {
          handleFeedMessage({ type: 'error_card', title: 'Error en delegación', exit_code: 502, stderr: res.error || 'Error inesperado' });
        }
        return;
      }

      if (e.target.id === 'err-retry-btn') {
        const q = runtime.errorCard?.retryPrompt || runtime.lastQ;
        if (q) {
          global.Bridge?.sendChat?.(q, curAg(), null);
          go('prompt');
        } else {
          go('overview');
        }
        return;
      }

      if (e.target.id === 'upd-check-btn') {
        await global.Bridge?.checkUpdate?.().catch(() => null);
        global.SoundFX?.chime?.();
        go('update_card', { keepState: true });
        return;
      }

      const mcpQuick = e.target.closest('[data-mcp-quick-install]');
      if (mcpQuick) {
        await global.Bridge?.installMcpCatalog?.(mcpQuick.dataset.mcpQuickInstall).catch(() => null);
        await global.Bridge?.fetchMcpCatalog?.().catch(() => []);
        go('mcp_download', { keepState: true });
        return;
      }

      const actEl = e.target.closest('[data-a]');
      if (actEl) {
        const act = actEl.dataset.a;
        if (act === 'toggle-mic') {
          if (global.MikuVoiceRecognizer) {
            global.MikuVoiceRecognizer.toggle();
          } else {
            runtime.voiceConsentOpen = !runtime.voiceConsentOpen;
          }
          global.SoundFX?.toggle?.();
          go(runtime.scene, { keepState: true });
        } else if (act === 'toggle-voice') {
          const nextVoice = !(CFG.voice !== false && CFG.sounds !== false);
          CFG.voice = nextVoice;
          CFG.sounds = nextVoice;
          global.SoundFX?.setEnabled?.(nextVoice);
          if (nextVoice) global.SoundFX?.toggle?.();
          global.Bridge?.saveSettings?.({ voice: nextVoice, sounds: nextVoice })?.catch?.(() => {});
          global.Bridge?.broadcast?.({ k: 'setting_updated', patch: { voice: nextVoice, sounds: nextVoice } });
          go(runtime.scene, { keepState: true });
        } else if (act === 'open-mascot') {
          window.open('./mascot.html', '_blank', 'noopener');
        } else if (act === 'island_hide') {
          collapseIsland();
        } else if (act === 'pair-refresh') {
          await global.Bridge?.generatePairingUrl?.().catch(() => null);
          go('link', { keepState: true });
        } else if (act === 'copy-pair') {
          try { await navigator.clipboard.writeText(runtime.pairUrl || ''); } catch (_) {}
          global.SoundFX?.chime?.();
        } else if (act === 'unpair') {
          await global.Bridge?.unpairDevices?.().catch(() => null);
          ST.link = 0;
          go('link', { keepState: true });
        } else if (act === 'copy-result') {
          try { await navigator.clipboard.writeText(runtime.resText || ''); } catch (_) {}
          global.SoundFX?.chime?.();
        }
      }
    });

    document.addEventListener('input', e => {
      if (e.target.id === 'wdr-color-pick') {
        const ag = curAg();
        if (!ag || ag === PRIN || ag.id === 'miku') return;
        const hex = e.target.value;
        if (usedByColor(hex, ag)) return;
        ag.c = hex;
        global.MikuMascot?.setColor?.(hex);
      }
    });

    document.addEventListener('change', e => {
      const qsKey = e.target?.dataset?.qsToggle;
      if (qsKey === 'voice') {
        const on = !!e.target.checked;
        CFG.voice = on ? 1 : 0;
        CFG.sounds = on;
        global.SoundFX?.setEnabled?.(on);
        saveCfg?.();
        global.Bridge?.saveSettings?.({ voice: on ? 1 : 0, sounds: on })?.catch?.(() => {});
        global.Bridge?.broadcast?.({ k: 'setting_updated', patch: { voice: on ? 1 : 0, sounds: on } });
        return;
      }
      if (qsKey === 'hover_open') {
        const val = e.target.checked ? 1 : 0;
        CFG.hover_open = val;
        saveCfg?.();
        global.Bridge?.setSetting?.('hover_open', val);
        global.Bridge?.broadcast?.({ k: 'setting_updated', patch: { hover_open: val } });
        return;
      }
      if (e.target.id === 'wdr-color-pick') {
        const ag = curAg();
        if (!ag || ag === PRIN || ag.id === 'miku') return;
        const hex = e.target.value;
        if (usedByColor(hex, ag)) return;
        ag.c = hex;
        saveAgents?.();
        syncMascotToAgent(ag);
        go('wardrobe', { keepState: true });
      } else if (e.target.id === 'wdr-tails-toggle') {
        global.MikuMascot?.setTwintails?.(e.target.checked);
        go('wardrobe', { keepState: true });
      }
    });

    isl.addEventListener('contextmenu', e => {
      if (e.target.closest('input,select,textarea')) return;
      if (!e.target.closest('#mk, .cs')) return;
      e.preventDefault();
      global.MikuMascot?.wakeFromRest?.();
      if (runtime.scene === 'wardrobe') {
        go(wBackScene || 'overview');
      } else {
        wBackScene = runtime.scene === 'compact' ? 'overview' : (runtime.scene || 'overview');
        go('wardrobe');
      }
    });

    if (fileInput) {
      fileInput.addEventListener('change', () => {
        if (fileInput.files?.[0]) handleSelectedFile(fileInput.files[0]);
        fileInput.value = '';
      });
    }

    // Drag & Drop interactivo hacia la mascota
    isl.addEventListener('dragover', e => {
      e.preventDefault();
      const r = isl.getBoundingClientRect();
      const nx = (e.clientX - r.left) / r.width * 2 - 1;
      const ny = (e.clientY - r.top) / r.height * 2 - 1;
      global.MikuMascot?.setEatProgress?.({ active: true, gx: nx, gy: ny, mouthOpen: 0.9 });
    });
    isl.addEventListener('dragleave', () => global.MikuMascot?.setEatProgress?.({ active: false, mouthOpen: 0 }));
    isl.addEventListener('drop', e => {
      e.preventDefault();
      global.MikuMascot?.setEatProgress?.({ active: false, mouthOpen: 0 });
      const file = e.dataTransfer?.files?.[0];
      if (file) handleSelectedFile(file);
    });

    document.addEventListener('keydown', e => {
      if (e.altKey && (e.code === 'Space' || e.key === ' ')) {
        e.preventDefault();
        global.MikuMascot?.wakeFromRest?.();
        const res = global.MikuVoiceRecognizer?.toggle?.();
        if (res?.started || res?.consentRequired) go('prompt', { keepState: true });
      } else if (e.key === 'Escape' && runtime.scene !== 'compact') {
        collapseIsland();
      } else if (runtime.scene === 'approve') {
        const k = e.key.toLowerCase();
        if (k === 'y') { global.Bridge?.choice?.(runtime.pendingApproveId, 'allow'); go('overview'); }
        else if (k === 'n') { global.Bridge?.choice?.(runtime.pendingApproveId, 'deny'); go('overview'); }
        else if (k === 'a') { global.Bridge?.choice?.(runtime.pendingApproveId, 'always'); go('overview'); }
      } else if (runtime.scene === 'question' && /^[1-9]$/.test(e.key)) {
        const idx = Number(e.key) - 1;
        const opts = runtime.question?.options || [];
        if (idx < opts.length) {
          global.SoundFX?.chime?.();
          global.Bridge?.sendUiChoice?.('question', opts[idx], idx, runtime.question?.title || '').catch(() => null);
          go('overview');
        }
      }
    });

    global.Bridge.onFeed(handleFeedMessage);
    global.Bridge.onBroadcast(msg => {
      if (msg?.k === 'mascot') {
        const target = (!msg.agentId || msg.agentId === 'miku') ? PRIN : ST.agents.find(x => x.id === msg.agentId);
        if (target && msg.color && target !== PRIN) target.c = msg.color;
        if (target && msg.outfit) target.outfit = msg.outfit;
        if (!target || target === curAg()) {
          if (msg.color) global.MikuMascot?.setColor?.(msg.color);
          if (msg.outfit) global.MikuMascot?.setOutfit?.(msg.outfit, !msg.agentId || msg.agentId === 'miku');
        }
        if (typeof msg.twintails === 'boolean') global.MikuMascot?.setTwintails?.(msg.twintails);
        if (msg.state) global.MikuMascot?.setState?.(msg.state);
        if (runtime.scene === 'wardrobe') go('wardrobe', { keepState: true });
      } else if (msg?.k === 'setting_updated' && msg.patch) {
        Object.assign(CFG, msg.patch);
        saveCfg?.();
        global.MikuUI.applyThemeVars?.(CFG);
        const voiceEnabled = typeof msg.patch.voice !== 'undefined' ? !!msg.patch.voice : CFG.sounds;
        if (typeof voiceEnabled === 'boolean') global.SoundFX?.setEnabled?.(voiceEnabled);
        if (typeof msg.patch.voice_volume === 'number') {
          MUS.vol = msg.patch.voice_volume;
          global.SoundFX?.setVolume?.(msg.patch.voice_volume);
        } else if (typeof CFG.vol === 'number') {
          global.SoundFX?.setVolume?.(CFG.vol);
        }
        if (typeof msg.patch.voice_wake_word_enabled !== 'undefined') {
          if (msg.patch.voice_wake_word_enabled) {
            global.MikuVoiceRecognizer?.startWakeWord?.();
          } else {
            global.MikuVoiceRecognizer?.stopWakeWord?.();
          }
        }
        const typing = runtime.editingDraft || document.activeElement?.id === 'qi' || document.activeElement?.tagName === 'INPUT' || document.activeElement?.tagName === 'TEXTAREA';
        if (msg.patch.lang) {
          (loadLocale?.(CFG.lang) || Promise.resolve()).then(() => {
            if (!typing) go(runtime.scene, { keepState: true });
          });
        } else if (!typing && !(runtime.scene === 'settings' && typeof msg.patch.voice_volume === 'number' && Object.keys(msg.patch).length === 1)) {
          go(runtime.scene, { keepState: true });
        }
      } else if (msg?.k === 'secrets_updated') {
        global.Bridge.syncAll?.();
      }
    });
    global.Bridge.connect(false);
    if (CFG.voice_wake_word_enabled && (CFG.voice_stt_enabled ?? true)) {
      setTimeout(() => { global.MikuVoiceRecognizer?.startWakeWord?.(); }, 1200);
    }

    setInterval(() => {
      if (document.hidden || !ST.ws) return;
      const onPlayerView =
        (runtime.scene === 'overview' && ((runtime.pg || 0) % 2 + 2) % 2 === 1) ||
        runtime.scene === 'player' ||
        runtime.scene === 'compact';
      if (onPlayerView) {
        global.Bridge?.fetchNowPlaying?.().catch(() => null);
      }
    }, 3000);

    if (typeof ResizeObserver === 'function' && innerEl) {
      let roRaf = 0;
      new ResizeObserver(() => {
        if (fittingNow) return;
        cancelAnimationFrame(roRaf);
        roRaf = requestAnimationFrame(fitIsland);
      }).observe(innerEl);
    }

    const urlParams = new URLSearchParams(window.location.search);
    const startOnboarding = urlParams.get('onboarding') === '1';
    const hashScene = window.location.hash.replace(/^#/, '');
    if (hashScene && global.MikuScenes.SC[hashScene]) {
      go(hashScene);
    } else {
      go('boot');
      LOCKI = 1;
      setTimeout(() => {
        LOCKI = 0;
        if (runtime.scene === 'boot') {
          go(startOnboarding ? 'settings' : (isWebViewHost ? 'compact' : 'overview'));
        }
      }, 950);
    }
  }

  global.feed = handleFeedMessage;
  global.MikuApp = { go, fit: fitIsland, ST, CFG, finalizeTurn };
  global.addEventListener('DOMContentLoaded', init);
})(window);
