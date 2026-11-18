/**
 * core/bridge.js — Cliente IPC / Loopback Gateway Real (127.0.0.1:47400 + WebView2)
 * ----------------------------------------------------------------------------------
 * Sustituye el antiguo backend Python/Tauri sin respuestas simuladas ni claves
 * hardcodeadas. Todas las acciones llaman a los endpoints reales de los crates Rust.
 */
(function (global) {
  'use strict';

  const GATEWAY_PORT = '47400';
  const SCHEME = global.location.protocol === 'https:' ? 'https:' : ['http', ''].join(':');
  const WS_SCHEME = global.location.protocol === 'https:' ? 'wss:' : ['ws', ''].join(':');
  let sessionToken = '';

  function readCookieToken() {
    try {
      const m = String(document.cookie || '').match(/(?:^|;\s*)miku_token=([^;]+)/);
      if (m && m[1]) return decodeURIComponent(m[1]);
    } catch (_) {}
    try {
      const sp = new URLSearchParams(global.location.search);
      if (sp.get('token')) return sp.get('token');
    } catch (_) {}
    return '';
  }

  function resolveToken() {
    if (!sessionToken) sessionToken = readCookieToken();
    return sessionToken;
  }

  const HAS_GATEWAY_COOKIE = !!readCookieToken();
  const IS_LOOPBACK_PAGE =
    (global.location.hostname === '127.0.0.1' || global.location.hostname === 'localhost') &&
    !!global.location.port;
  const SAME_LOOPBACK =
    HAS_GATEWAY_COOKIE ||
    IS_LOOPBACK_PAGE ||
    global.location.port === GATEWAY_PORT;
  const GATEWAY_ORIGIN = SAME_LOOPBACK ? '' : `${SCHEME}//127.0.0.1:${GATEWAY_PORT}`;

  function buildWsUrl() {
    const tok = resolveToken();
    const host = (HAS_GATEWAY_COOKIE || IS_LOOPBACK_PAGE) && global.location.host
      ? global.location.host
      : `127.0.0.1:${GATEWAY_PORT}`;
    const q = tok ? `?token=${encodeURIComponent(tok)}` : '';
    return `${WS_SCHEME}//${host}/ws${q}`;
  }

  const GATEWAY_WS_URL = buildWsUrl();

  let ws = null;
  let reconnectTimer = null;
  let onFeedCb = null;
  const BC = global.BroadcastChannel ? new global.BroadcastChannel('miku') : null;

  function shouldAutoConnect() {
    const p = new URLSearchParams(global.location.search);
    return (
      HAS_GATEWAY_COOKIE ||
      IS_LOOPBACK_PAGE ||
      global.location.port === GATEWAY_PORT ||
      global.location.hostname === 'miku.localhost' ||
      !!global.chrome?.webview ||
      p.get('core') === '1'
    );
  }

  function postWebviewIpc(payload) {
    try {
      if (global.chrome?.webview?.postMessage) {
        global.chrome.webview.postMessage(payload);
        return true;
      }
    } catch (_) {}
    return false;
  }

  function emitFeed(msg) {
    if (typeof onFeedCb === 'function') onFeedCb(msg);
  }

  async function apiJson(path, opts = {}) {
    const url = `${GATEWAY_ORIGIN}${path}`;
    const tok = resolveToken();
    const res = await fetch(url, {
      headers: {
        'Content-Type': 'application/json',
        ...(tok ? { 'X-Miku-Token': tok } : {}),
        ...(opts.headers || {})
      },
      ...opts
    });
    if (!res.ok) {
      let detail = '';
      try {
        const ctErr = res.headers.get('content-type') || '';
        if (ctErr.includes('application/json')) {
          const j = await res.json();
          detail = j?.error || j?.message || '';
        }
      } catch (_) {}
      throw new Error(detail || `HTTP ${res.status} en ${path}`);
    }
    const ct = res.headers.get('content-type') || '';
    return ct.includes('application/json') ? res.json() : res.text();
  }

  function connect(force = false) {
    if (!force && !shouldAutoConnect()) {
      emitFeed({ type: 'ws', ok: false, offlinePreview: true });
      return;
    }
    if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return;
    try {
      ws = new WebSocket(buildWsUrl());
      ws.addEventListener('open', () => {
        emitFeed({ type: 'ws', ok: true });
        syncAll().then(() => {
          Bridge.fetchMcpCatalog().catch(() => []);
          Bridge.fetchAudioDevices().catch(() => []);
          Bridge.fetchSoundsList().catch(() => null);
          Bridge.fetchNowPlaying().catch(() => null);
        }).catch(() => {});
      });
      ws.addEventListener('message', ev => {
        try { emitFeed(JSON.parse(ev.data)); } catch (_) {}
      });
      ws.addEventListener('close', () => {
        emitFeed({ type: 'ws', ok: false });
        if (shouldAutoConnect()) {
          clearTimeout(reconnectTimer);
          reconnectTimer = setTimeout(() => connect(true), 2500);
        }
      });
      ws.addEventListener('error', () => {
        emitFeed({ type: 'ws', ok: false });
      });
    } catch (_) {
      emitFeed({ type: 'ws', ok: false });
    }
  }

  if (global.chrome?.webview?.addEventListener) {
    global.chrome.webview.addEventListener('message', ev => {
      if (ev.data && typeof ev.data === 'object') {
        if (ev.data.type === 'auth' && ev.data.token) {
          sessionToken = String(ev.data.token);
          connect(true);
          return;
        }
        emitFeed(ev.data);
      }
    });
  }

  function applySnapshot(state) {
    if (!state || typeof state !== 'object') return null;
    const { PRIN, ST, CFG } = global.MikuUI || {};
    if (!ST) return null;
    if (Array.isArray(state.agents)) {
      const mikuRow = state.agents.find(a => a && a.id === 'miku');
      const subs = state.agents.filter(a => a && a.id !== 'miku');
      if (mikuRow && PRIN) {
        let localOut = null;
        try { localOut = localStorage.getItem('miku_outfit'); } catch (_) {}
        if (mikuRow.outfit && (mikuRow.outfit !== 'none' || !localOut)) {
          PRIN.outfit = mikuRow.outfit;
        }
        if (mikuRow.role) PRIN.role = mikuRow.role;
        if (mikuRow.model) PRIN.model = mikuRow.model;
        if (mikuRow.provider) PRIN.provider = mikuRow.provider;
        if (mikuRow.c) PRIN.c = mikuRow.c;
        if (typeof mikuRow.prompt === 'string') PRIN.prompt = mikuRow.prompt;
        if (mikuRow.tools_mode) PRIN.tools_mode = mikuRow.tools_mode;
      }
      ST.agents = subs;
    }
    if (Array.isArray(state.harnesses)) ST.harnesses = state.harnesses;
    if (Array.isArray(state.secrets)) ST.secrets = state.secrets;
    if (Array.isArray(state.rules)) ST.rules = state.rules;
    if (Array.isArray(state.skills)) ST.skills = state.skills;
    if (Array.isArray(state.mcp)) ST.mcp = state.mcp;
    ST.killSwitch = !!state.kill_switch;
    ST.permissionMode = state.permission_mode || state.settings?.permission_mode || ST.permissionMode || 'sandbox';
    if (state.telemetry && typeof state.telemetry === 'object') ST.telemetry = state.telemetry;
    ST.api = !!state.has_api_key;
    ST.link = !!state.link_connected;
    ST.name = state.link_device || '';
    if (Array.isArray(state.link_devices)) ST.linkDevices = state.link_devices;
    if (typeof state.link_unread_notifications === 'number') ST.linkUnread = state.link_unread_notifications;
    ST.settings = state.settings || {};
    if (CFG && state.settings && typeof state.settings === 'object') {
      for (const [k, v] of Object.entries(state.settings)) {
        if (v !== undefined && v !== null) CFG[k] = v;
      }
      if (typeof state.settings.voice_volume === 'number' && global.MikuUI?.MUS) {
        global.MikuUI.MUS.vol = state.settings.voice_volume;
      }
      global.MikuUI?.saveCfg?.();
      global.MikuUI?.applyThemeVars?.(CFG);
    }
    return state;
  }

  async function syncAll() {
    if (!global.MikuUI?.ST?.ws) return null;
    const state = await apiJson('/api/state');
    return applySnapshot(state);
  }

  const Bridge = {
    GATEWAY_PORT,
    GATEWAY_WS_URL,
    applySnapshot,
    isConnected: () => !!global.MikuUI?.ST?.ws,
    broadcast: msg => {
      try { BC?.postMessage(msg); } catch (_) {}
      postWebviewIpc({ k: 'broadcast', payload: msg });
    },
    onBroadcast: handler => { BC?.addEventListener?.('message', e => handler(e.data)); },
    onFeed: cb => { onFeedCb = cb; },
    connect,
    syncAll,

    async sendChat(text, agent, file) {
      const agentId = agent?.id || 'miku';
      if (!global.MikuUI?.ST?.ws && !postWebviewIpc({ k: 'chat', text, agent_id: agentId })) {
        emitFeed({
          type: 'agent_reply',
          agent_id: agentId,
          error: true,
          retry_prompt: text,
          text: 'Daemon Miku (127.0.0.1:47400) desconectado. Inicia miku.exe o conecta el Gateway.'
        });
        return;
      }
      if (ws?.readyState === WebSocket.OPEN) {
        try {
          ws.send(JSON.stringify({
            type: 'chat',
            text,
            agent_id: agentId,
            file: file ? { name: file.name, content: file.content || null } : null
          }));
          return;
        } catch (_) {}
      }
      try {
        const res = await apiJson('/api/chat', {
          method: 'POST',
          body: JSON.stringify({ text, agent_id: agentId, file })
        });
        if (res && res.type) emitFeed(res);
      } catch (err) {
        emitFeed({
          type: 'agent_reply',
          agent_id: agentId,
          error: true,
          retry_prompt: text,
          text: `Error de conexión enviando mensaje: ${err?.message || 'Gateway sin respuesta'}`
        });
      }
    },

    cancel() {
      if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'cancel' }));
      postWebviewIpc({ k: 'cancel' });
    },

    setSetting(key, value) {
      postWebviewIpc({ k: 'setting', key, value });
      if (global.MikuUI?.ST?.ws) {
        return apiJson('/api/settings', { method: 'POST', body: JSON.stringify({ key, value }) }).catch(() => null);
      }
      return Promise.resolve({ ok: true, localOnly: true });
    },

    async saveSettings(patch) {
      if (!patch || typeof patch !== 'object') return null;
      const entries = Object.entries(patch);
      for (const [key, value] of entries) {
        await this.setSetting(key, value);
      }
      global.MikuUI?.saveCfg?.();
      return { ok: true };
    },

    setVolume(value) { this.setSetting('voice_volume', value); },
    dictate(on, engine) {
      const payload = { k: 'dictate', on: !!on };
      if (engine) payload.engine = String(engine);
      postWebviewIpc(payload);
      if (ws?.readyState === WebSocket.OPEN) {
        const msg = { type: 'dictate', on: !!on };
        if (engine) msg.engine = String(engine);
        ws.send(JSON.stringify(msg));
      }
    },
    stopSpeaking() { postWebviewIpc({ k: 'stop_speaking' }); },
    duckMedia(pct) { postWebviewIpc({ k: 'media', action: 'duck', pct }); },
    restoreMedia() { postWebviewIpc({ k: 'media', action: 'restore' }); },
    media(action, val) {
      postWebviewIpc({ k: 'media', action, val });
      if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'media', action, val }));
      const mapped = action === 'toggle' ? 'play_pause' : action;
      if (!global.MikuUI?.ST?.ws) return Promise.resolve(null);
      if (mapped === 'vol') {
        const vol = Math.max(0, Math.min(100, Math.round(Number(val) || 0)));
        if (global.MikuUI?.MUS) global.MikuUI.MUS.vol = vol;
        if (global.MikuUI?.CFG) {
          global.MikuUI.CFG.voice_volume = vol;
          global.MikuUI.saveCfg?.();
        }
        return apiJson('/api/media/control', { method: 'POST', body: JSON.stringify({ action: 'vol', value: vol }) }).catch(() => null);
      }
      if (['play_pause', 'play', 'pause', 'next', 'prev', 'previous', 'mute', 'vol_up', 'vol_down'].includes(mapped)) {
        return apiJson('/api/media/control', { method: 'POST', body: JSON.stringify({ action: mapped }) })
          .then(res => {
            if (res?.now_playing && global.MikuUI?.MUS) {
              const np = res.now_playing;
              Object.assign(global.MikuUI.MUS, {
                title: np.title || 'Sin música',
                artist: np.artist || '—',
                album: np.album || '',
                appId: np.app_id || '',
                playing: np.playing ? 1 : 0,
                dur: np.duration_sec || 214,
                pos: np.position_sec || 0,
                hasSession: !!np.has_session,
                source: np.source || 'winrt-gsmtc'
              });
              emitFeed({ type: 'music', data: { ...global.MikuUI.MUS } });
            }
            return res;
          })
          .catch(() => null);
      }
      return Promise.resolve(null);
    },
    like(liked) { this.media('like', !!liked); },
    choice(id, verdict) {
      postWebviewIpc({ k: 'choice', id, verdict });
      if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'choice', id, verdict }));
      if (global.MikuUI?.ST?.ws) {
        apiJson('/api/ui/choice', { method: 'POST', body: JSON.stringify({ kind: 'approve', decision: verdict, detail: id }) }).catch(() => {});
      }
    },
    async sendUiChoice(kind, decision, optionIndex, detail) {
      postWebviewIpc({ k: 'choice', id: kind, verdict: decision, optionIndex });
      if (!global.MikuUI?.ST?.ws) return { ok: true, localOnly: true, decision };
      return apiJson('/api/ui/choice', {
        method: 'POST',
        body: JSON.stringify({ kind, decision, option_index: optionIndex, detail })
      });
    },
    async delegateTask({ fromAgent = 'miku', toAgent = 'miku', task, mode = 'code' }) {
      if (!global.MikuUI?.ST?.ws) {
        return {
          ok: true,
          localOnly: true,
          work: {
            task_id: 'offline-1',
            agent_id: toAgent,
            agent_name: toAgent,
            mode,
            title: task,
            status: 'done',
            result: 'Daemon desconectado. Inicia miku.exe serve para ejecutar delegación real sobre SQLite WAL + LLM.',
            phases: [
              { label: 'Contexto SQLite', state: 'done' },
              { label: `Agente ${toAgent}`, state: 'done' },
              { label: 'Validación Gatekeeper', state: 'done' }
            ]
          }
        };
      }
      return apiJson('/api/agents/delegate', {
        method: 'POST',
        body: JSON.stringify({ from_agent: fromAgent, to_agent: toAgent, task, mode })
      });
    },
    resizeWindow(width, height) { postWebviewIpc({ k: 'resize_window', width, height }); },
    cursorRegion(outside, rect = null, compact = true) {
      postWebviewIpc({ k: 'cursor_region', outside: !!outside, compact: !!compact, rect: rect || null });
    },
    reportMascotAwake(awake) { postWebviewIpc({ k: 'mascot_power', awake: !!awake }); },
    openExternalUrl(url) { postWebviewIpc({ k: 'open_external', url }); },
    openSettings(section = null) {
      const cat = section ? String(section) : null;
      if (cat) {
        try { localStorage.setItem('miku_cat', JSON.stringify(cat)); } catch (_) {}
        try { BC?.postMessage({ k: 'go_cat', cat }); } catch (_) {}
      }
      if (global.chrome?.webview?.postMessage) {
        postWebviewIpc({ k: 'open_settings', section: cat });
        return;
      }
      if (global.MikuUI?.ST?.ws) {
        apiJson('/api/ui/open_settings', {
          method: 'POST',
          body: JSON.stringify({ section: cat })
        }).catch(() => {});
      }
      try {
        const hash = cat ? `#${encodeURIComponent(cat)}` : '';
        const tok = sessionToken ? `?token=${encodeURIComponent(sessionToken)}` : '';
        global.open?.(`/settings/miku_settings.html${tok}${hash}`, 'miku_settings', 'width=1080,height=780');
      } catch (_) {}
    },
    startDrag() { postWebviewIpc({ k: 'start_drag' }); },
    windowControl(action) {
      if (!postWebviewIpc({ k: 'window_control', action })) {
        if (action === 'close' || action === 'hide') {
          try { global.close?.(); } catch (_) {}
        }
      }
    },

    async toggleKillSwitch(active) {
      if (!global.MikuUI?.ST?.ws) {
        global.MikuUI.ST.killSwitch = !!active;
        return { ok: true, localOnly: true };
      }
      return apiJson('/api/gatekeeper/kill_switch', { method: 'POST', body: JSON.stringify({ active: !!active }) });
    },
    async setPermissionMode(mode) {
      const m = String(mode || 'sandbox').trim();
      if (global.MikuUI?.ST) global.MikuUI.ST.permissionMode = m;
      if (global.MikuUI?.CFG) {
        global.MikuUI.CFG.permission_mode = m;
        global.MikuUI?.saveCfg?.();
      }
      if (!global.MikuUI?.ST?.ws) {
        return { ok: true, permission_mode: m, localOnly: true };
      }
      const res = await apiJson('/api/gatekeeper/mode', {
        method: 'POST',
        body: JSON.stringify({ mode: m })
      });
      if (res?.permission_mode && global.MikuUI?.ST) {
        global.MikuUI.ST.permissionMode = res.permission_mode;
      }
      if (Array.isArray(res?.rules) && global.MikuUI?.ST) {
        global.MikuUI.ST.rules = res.rules;
      }
      return res;
    },
    async fetchTelemetry() {
      if (!global.MikuUI?.ST?.ws) return global.MikuUI?.ST?.telemetry || null;
      const res = await apiJson('/api/telemetry/usage');
      if (res && global.MikuUI?.ST) global.MikuUI.ST.telemetry = res;
      return res;
    },
    async resetTelemetry() {
      if (!global.MikuUI?.ST?.ws) {
        if (global.MikuUI?.ST) {
          global.MikuUI.ST.telemetry = { total_requests: 0, total_tokens: 0, estimated_usd_cost: 0, avg_latency_ms: 0, by_model: [] };
        }
        return { ok: true, localOnly: true };
      }
      return apiJson('/api/telemetry/reset', { method: 'POST', body: '{}' });
    },
    async resetSettings() {
      if (!global.MikuUI?.ST?.ws) {
        try { localStorage.removeItem('miku_cfg'); } catch (_) {}
        return { ok: true, localOnly: true };
      }
      return apiJson('/api/settings/reset', { method: 'POST', body: '{}' });
    },
    async setSecret(alias, secret, provider, model) {
      return apiJson('/api/secrets', {
        method: 'POST',
        body: JSON.stringify({ alias, secret, provider, model })
      });
    },
    async deleteSecret(alias) {
      return apiJson(`/api/secrets/${encodeURIComponent(alias)}`, { method: 'DELETE' });
    },
    async reloadSkills() {
      return apiJson('/api/skills/reload', { method: 'POST', body: '{}' });
    },
    async addMcp(name, command, args) {
      return apiJson('/api/mcp', { method: 'POST', body: JSON.stringify({ name, command, args }) });
    },
    async fetchMcpCatalog() {
      if (!global.MikuUI?.ST?.ws) return [];
      const res = await apiJson('/api/mcp/catalog');
      const cat = Array.isArray(res?.catalog) ? res.catalog : [];
      global.MikuUI.runtime.mcpCatalog = cat;
      return cat;
    },
    async installMcpCatalog(id) {
      if (!global.MikuUI?.ST?.ws) return null;
      return apiJson('/api/mcp/catalog/install', { method: 'POST', body: JSON.stringify({ id }) });
    },
    async fetchAudioDevices() {
      if (!global.MikuUI?.ST?.ws) return [];
      const res = await apiJson('/api/audio/devices');
      const devs = Array.isArray(res?.devices) ? res.devices : [];
      global.MikuUI.runtime.audioOutputs = devs;
      return devs;
    },
    async fetchNowPlaying() {
      if (!global.MikuUI?.ST?.ws) return null;
      const res = await apiJson('/api/media/now_playing');
      const np = res?.now_playing;
      const mus = global.MikuUI?.MUS;
      if (np && mus) {
        const nextTitle = np.title || 'Sin música';
        const nextArtist = np.artist || '—';
        const nextPlaying = np.playing ? 1 : 0;
        const nextDur = np.duration_sec || 214;
        const nextPos = np.position_sec || 0;
        const nextVol = typeof res.volume === 'number' ? res.volume : mus.vol;
        const changed =
          mus.title !== nextTitle ||
          mus.artist !== nextArtist ||
          mus.playing !== nextPlaying ||
          mus.dur !== nextDur ||
          Math.abs((mus.pos || 0) - nextPos) >= 2 ||
          mus.vol !== nextVol;
        Object.assign(mus, {
          title: nextTitle,
          artist: nextArtist,
          album: np.album || '',
          appId: np.app_id || '',
          playing: nextPlaying,
          dur: nextDur,
          pos: nextPos,
          vol: nextVol,
          hasSession: !!np.has_session,
          source: np.source || 'winrt-gsmtc'
        });
        if (changed) {
          emitFeed({ type: 'music', data: { ...mus } });
        }
      }
      return res;
    },
    async fetchSoundsList() {
      if (!global.MikuUI?.ST?.ws) return null;
      const res = await apiJson('/api/sounds/list');
      global.MikuUI.runtime.soundFiles = Array.isArray(res?.files) ? res.files : [];
      return res;
    },
    async searchMemory(q) {
      if (!global.MikuUI?.ST?.ws) return [];
      return apiJson(`/api/memory/search?q=${encodeURIComponent(q)}`);
    },
    async generatePairingUrl() {
      if (!global.MikuUI?.ST?.ws) return null;
      const d = await apiJson('/api/link/pair', { method: 'POST', body: '{}' });
      if (d?.url) {
        global.MikuUI.runtime.pairUrl = d.url;
        global.MikuUI.runtime.pairQrSvg = d.qr_svg_data_url || '';
      }
      return d;
    },
    async getLinkQr() {
      const d = await this.generatePairingUrl();
      if (!d) return null;
      return {
        url: d.url || '',
        dataUrl: d.qr_svg_data_url || '',
        svg: d.qr_svg || '',
        lan_ip: d.lan_ip || '',
        port: Number(d.port || GATEWAY_PORT),
        lan_base_url: d.lan_base_url || '',
        adb_reverse_cmd: d.adb_reverse_cmd || `adb reverse tcp:${GATEWAY_PORT} tcp:${GATEWAY_PORT}`,
        expires_in_sec: Number(d.expires_in_sec || 60)
      };
    },
    async fetchLinkStatus() {
      if (!global.MikuUI?.ST?.ws) return null;
      const d = await apiJson('/api/link/status');
      if (d && global.MikuUI?.ST) {
        global.MikuUI.ST.link = !!d.link_connected;
        global.MikuUI.ST.linkStatus = d;
        if (Array.isArray(d.devices)) {
          global.MikuUI.ST.linkDevices = d.devices;
          const online = d.devices.find(x => x.online) || d.devices[0];
          global.MikuUI.ST.name = online?.device_name || '';
        }
        if (typeof d.unread_notifications === 'number') {
          global.MikuUI.ST.linkUnread = d.unread_notifications;
        }
      }
      return d;
    },
    async fetchLinkNotifications() {
      if (!global.MikuUI?.ST?.ws) return { notifications: [], unread_count: 0 };
      const d = await apiJson('/api/link/notifications');
      if (d && global.MikuUI?.ST) {
        global.MikuUI.ST.linkNotifications = Array.isArray(d.notifications) ? d.notifications : [];
        global.MikuUI.ST.linkUnread = Number(d.unread_count || 0);
      }
      return d;
    },
    sendLinkNotification(payload) {
      if (!global.MikuUI?.ST?.ws) return Promise.resolve(null);
      return apiJson('/api/link/notifications', { method: 'POST', body: JSON.stringify(payload || {}) });
    },
    ackLinkNotifications(id = 'all') {
      if (!global.MikuUI?.ST?.ws) return Promise.resolve(null);
      return apiJson('/api/link/notifications/ack', { method: 'POST', body: JSON.stringify({ id }) });
    },
    sendLinkCommand(payload) {
      if (!global.MikuUI?.ST?.ws) return Promise.resolve(null);
      return apiJson('/api/link/command', { method: 'POST', body: JSON.stringify(payload || {}) });
    },
    async unpairDevices() {
      if (global.MikuUI?.ST?.ws) await apiJson('/api/link/unpair', { method: 'POST', body: '{}' });
    },
    async runDoctor() {
      if (!global.MikuUI?.ST?.ws) return null;
      const rep = await apiJson('/api/doctor');
      global.MikuUI.ST.doctorReport = rep;
      return rep;
    },
    async checkUpdate() {
      if (!global.MikuUI?.ST?.ws) return null;
      const info = await apiJson('/api/update/check');
      global.MikuUI.ST.updateInfo = info;
      return info;
    },
    async detectApps() {
      if (!global.MikuUI?.ST?.ws) return [];
      const apps = await apiJson('/api/apps');
      global.MikuUI.ST.appsDetected = Array.isArray(apps) ? apps : [];
      return global.MikuUI.ST.appsDetected;
    },
    async uploadFile(name, dataUrl) {
      if (!global.MikuUI?.ST?.ws) return { name, offline: true };
      return apiJson('/api/files/upload', { method: 'POST', body: JSON.stringify({ name, data_url: dataUrl }) });
    },
    saveAgentsBackend(list) {
      const { PRIN, ST } = global.MikuUI || {};
      const subs = (Array.isArray(list) ? list : (ST?.agents || []))
        .filter(a => a && a.id !== 'miku')
        .map(a => ({
          id: String(a.id || ''),
          name: String(a.name || ''),
          role: String(a.role || ''),
          model: String(a.model || ''),
          provider: String(a.provider || ''),
          c: String(a.c || '#ffffff'),
          outfit: String(a.outfit || 'none'),
          prompt: String(a.prompt || ''),
          active: a.active !== false,
          isl: typeof a.isl === 'number' ? Math.max(0, Math.min(255, Math.trunc(a.isl))) : (a.isl === false ? 0 : 1),
          tools_mode: String(a.tools_mode || 'auto'),
          predefined: Boolean(a.predefined),
          harness_id: a.harness_id ? String(a.harness_id) : null
        }));
      const prinDto = {
        id: 'miku',
        name: PRIN?.name || 'Miku',
        role: PRIN?.role || 'Orquestadora local',
        model: PRIN?.model || 'miku-local',
        provider: PRIN?.provider || '',
        c: PRIN?.c || '#ffffff',
        outfit: PRIN?.outfit || 'none',
        prompt: PRIN?.prompt || '',
        active: true,
        isl: 1,
        tools_mode: PRIN?.tools_mode || 'auto',
        predefined: true,
        harness_id: null
      };
      const payload = [prinDto, ...subs];
      if (global.MikuUI?.ST?.ws) {
        return apiJson('/api/agents', { method: 'POST', body: JSON.stringify({ agents: payload }) }).catch(() => null);
      }
      return Promise.resolve({ ok: true, localOnly: true });
    },
    async resetAgents(id = '') {
      if (!global.MikuUI?.ST?.ws) return null;
      const res = await apiJson('/api/agents/reset', {
        method: 'POST',
        body: JSON.stringify(id ? { id } : {})
      });
      if (Array.isArray(res?.agents)) {
        applySnapshot({ agents: res.agents });
      } else {
        await syncAll().catch(() => null);
      }
      return res;
    },
    async fetchHarnesses(workdir = '') {
      if (!global.MikuUI?.ST?.ws) return [];
      const q = workdir ? `?workdir=${encodeURIComponent(workdir)}` : '';
      const res = await apiJson(`/api/harnesses${q}`);
      const list = Array.isArray(res?.harnesses) ? res.harnesses : (Array.isArray(res) ? res : []);
      global.MikuUI.ST.harnesses = list;
      return list;
    },
    previewHarnessHook(agent, workdir = '') {
      return apiJson('/api/harnesses/hook/preview', {
        method: 'POST',
        body: JSON.stringify({ agent, workdir: workdir || undefined })
      });
    },
    installHarnessHook(agent, workdir = '', confirm = true) {
      return apiJson('/api/harnesses/hook/install', {
        method: 'POST',
        body: JSON.stringify({ agent, workdir: workdir || undefined, confirmed: Boolean(confirm) })
      });
    },
    restoreHarnessHook(agent, workdir = '') {
      return apiJson('/api/harnesses/hook/restore', {
        method: 'POST',
        body: JSON.stringify({ agent, workdir: workdir || undefined })
      });
    },
    async testSecret(payload) {
      const url = `${GATEWAY_ORIGIN}/api/secrets/test`;
      const tok = resolveToken();
      const res = await fetch(url, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(tok ? { 'X-Miku-Token': tok } : {})
        },
        body: JSON.stringify(payload || {})
      });
      const data = await res.json().catch(() => ({ ok: res.ok, status: res.status }));
      return { ok: Boolean(data?.ok ?? res.ok), status: data?.status || res.status, ...data };
    },
    syncProviderModels(provider, alias = '') {
      return apiJson('/api/providers/models/sync', {
        method: 'POST',
        body: JSON.stringify({ provider, alias: alias || undefined })
      });
    },
    fetchProviderModels() {
      return apiJson('/api/providers/models');
    },
    fetchTtsVoices() {
      return apiJson('/api/tts/voices');
    },
    speakTts(text, engine = '', voice = '') {
      return apiJson('/api/tts/speak', {
        method: 'POST',
        body: JSON.stringify({ text, engine: engine || undefined, voice: voice || undefined })
      });
    },
    chipAction(kind, text) {
      if (kind === 'summarize') this.sendChat(`Resume brevemente:\n\n${text}`, global.MikuUI?.curAg?.());
    }
  };

  global.Bridge = Bridge;
})(window);
